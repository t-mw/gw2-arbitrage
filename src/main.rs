use bincode::{deserialize_from, serialize_into};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use futures::{stream, StreamExt};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use std::fs::File;
use std::path::Path;

const MAX_PAGE_SIZE: i32 = 200; // https://wiki.guildwars2.com/wiki/API:2#Paging
const MAX_ITEM_ID_LENGTH: i32 = 200; // error returned for greater than this amount
const TRADING_POST_COMMISSION: f32 = 0.15;

const PARALLEL_REQUESTS: usize = 10;

#[derive(Debug, Serialize, Deserialize)]
struct Price {
    id: u32,
    buys: PriceInfo,
    sells: PriceInfo,
}

impl Price {
    fn effective_buy_price(&self) -> i32 {
        (self.buys.unit_price as f32 * (1.0 - TRADING_POST_COMMISSION)).floor() as i32
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PriceInfo {
    unit_price: i32,
    quantity: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Recipe {
    id: u32,
    #[serde(rename = "type")]
    type_name: String,
    output_item_id: u32,
    output_item_count: i32,
    time_to_craft_ms: i32,
    disciplines: Vec<String>,
    min_rating: i32,
    flags: Vec<String>,
    ingredients: Vec<RecipeIngredient>,
    chat_link: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecipeIngredient {
    item_id: u32,
    count: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Item {
    id: u32,
    chat_link: String,
    name: String,
    icon: Option<String>,
    description: Option<String>,
    #[serde(rename = "type")]
    type_name: String,
    rarity: String,
    level: i32,
    vendor_value: i32,
    default_skin: Option<i32>,
    flags: Vec<String>,
    game_types: Vec<String>,
    restrictions: Vec<String>,
    upgrades_into: Option<Vec<ItemUpgrade>>,
    upgrades_from: Option<Vec<ItemUpgrade>>,
}

impl Item {
    fn vendor_cost(&self) -> Option<i32> {
        let name = &self.name;

        if name.starts_with("Thermocatalytic")
            || (name.starts_with("Spool of")
                && name.ends_with("Thread")
                && !name.starts_with("Spool of Deldrimor"))
            || (name.ends_with("of Holding") && !name.starts_with("Supreme"))
            || name.starts_with("Lump of")
            || name == "Jar of Vinegar"
            || name == "Packet of Baking Powder"
            || name == "Jar of Vegetable Oil"
            || name == "Packet of Salt"
            || name == "Bag of Sugar"
            || name == "Jug of Water"
            || name == "Bag of Starch"
            || name == "Bag of Flour"
            || name == "Bottle of Soy Sauce"
            || name == "Milling Basin"
        {
            if self.vendor_value > 0 {
                // standard vendor sell price is generally buy price * 8, see:
                //  https://forum-en.gw2archive.eu/forum/community/api/How-to-get-the-vendor-sell-price
                Some(self.vendor_value * 8)
            } else {
                None
            }
        } else if name == "Pile of Compost Starter" {
            Some(150)
        } else if name == "Pile of Powdered Gelatin Mix" {
            Some(200)
        } else if name == "Smell-Enhancing Culture" {
            Some(40000)
        } else {
            None
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ItemUpgrade {
    upgrade: String,
    item_id: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ItemListings {
    id: u32,
    buys: Vec<Listing>,
    sells: Vec<Listing>,
}

impl ItemListings {
    fn calculate_crafting_profit(
        &mut self,
        recipes_map: &FxHashMap<u32, Recipe>,
        items_map: &FxHashMap<u32, Item>,
        mut tp_listings_map: FxHashMap<u32, ItemListings>,
    ) -> ProfitableItem {
        let mut listing_profit = 0;
        let mut total_crafting_cost = CraftingCost { cost: 0, count: 0 };

        loop {
            let crafting_cost = if let Some(crafting_cost) = calculate_precise_min_crafting_cost(
                self.id,
                recipes_map,
                items_map,
                &mut tp_listings_map,
            ) {
                crafting_cost
            } else {
                break;
            };

            // NB: introduces small error due to integer division
            let unit_crafting_cost = crafting_cost.cost / crafting_cost.count;

            for _ in 0..crafting_cost.count {
                let buy_price = if let Some(buy_price) = self.sell() {
                    buy_price
                } else {
                    break;
                };

                let profit = buy_price - unit_crafting_cost;
                if profit > 0 {
                    listing_profit += profit;

                    total_crafting_cost.cost += unit_crafting_cost;
                    total_crafting_cost.count += 1;
                } else {
                    break;
                }
            }
        }

        ProfitableItem {
            id: self.id,
            crafting_cost: total_crafting_cost,
            profit: listing_profit,
        }
    }

    fn buy(&mut self, mut count: i32) -> Option<i32> {
        let mut cost = 0;

        while count > 0 {
            // sells are sorted in descending price
            let remove = if let Some(listing) = self.sells.last_mut() {
                listing.quantity -= 1;

                count -= 1;
                cost += listing.unit_price;

                listing.quantity == 0
            } else {
                return None;
            };

            if remove {
                self.sells.pop();
            }
        }

        Some(cost)
    }

    fn sell(&mut self) -> Option<i32> {
        let mut revenue = 0;

        // buys are sorted in ascending price
        let remove = if let Some(listing) = self.buys.last_mut() {
            listing.quantity -= 1;

            revenue += listing.unit_price_minus_fees();

            listing.quantity == 0
        } else {
            return None;
        };

        if remove {
            self.buys.pop();
        }

        Some(revenue)
    }

    fn lowest_sell_offer(&self, count: i32) -> Option<i32> {
        let len = self.sells.len();
        if len < count as usize {
            return None;
        }

        let slice = &self.sells[len - count as usize..len];
        Some(slice.iter().map(|listing| listing.unit_price).sum())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Listing {
    listings: i32,
    unit_price: i32,
    quantity: i32,
}

impl Listing {
    fn unit_price_minus_fees(&self) -> i32 {
        (self.unit_price as f32 * (1.0 - TRADING_POST_COMMISSION)).floor() as i32
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prices_path = "prices.bin";
    let recipes_path = "recipes.bin";
    let items_path = "items.bin";

    println!("Loading trading post prices");
    let tp_prices: Vec<Price> = ensure_paginated_cache(prices_path, "commerce/prices").await?;
    println!("Loaded {} trading post prices", tp_prices.len());

    println!("Loading recipes");
    let recipes: Vec<Recipe> = ensure_paginated_cache(recipes_path, "recipes").await?;
    println!("Loaded {} recipes", recipes.len());

    println!("Loading items");
    let items: Vec<Item> = ensure_paginated_cache(items_path, "items").await?;
    println!("Loaded {} items", items.len());

    let tp_prices_map = vec_to_map(tp_prices, |x| x.id);
    let recipes_map = vec_to_map(recipes, |x| x.output_item_id);
    let items_map = vec_to_map(items, |x| x.id);

    let mut profitable_item_ids = vec![];
    let mut ingredient_ids = vec![];
    for (item_id, _) in &recipes_map {
        if let Some(crafting_cost) = calculate_estimated_min_crafting_cost(
            *item_id,
            &recipes_map,
            &items_map,
            &tp_prices_map,
        ) {
            // some items are craftable and have no listed restrictions but are still not listable on tp
            // e.g. https://api.guildwars2.com/v2/items/39417
            if let Some(tp_prices) = tp_prices_map.get(item_id) {
                if tp_prices.effective_buy_price() * crafting_cost.count > crafting_cost.cost {
                    profitable_item_ids.push(*item_id);
                    collect_ingredient_ids(*item_id, &recipes_map, &mut ingredient_ids);
                }
            }
        }
    }

    println!("Loading detailed trading post listings");
    let mut request_listing_item_ids = vec![];
    request_listing_item_ids.extend(&profitable_item_ids);
    request_listing_item_ids.extend(ingredient_ids);
    request_listing_item_ids.sort_unstable();
    request_listing_item_ids.dedup();

    let mut tp_listings: Vec<ItemListings> =
        request_item_ids("commerce/listings", &request_listing_item_ids).await?;
    for listings in &mut tp_listings {
        // by default sells are listed in ascending price.
        // reverse list to allow lowest sells to be popped instead of spliced from front.
        listings.sells.reverse();
    }
    println!(
        "Loaded {} detailed trading post listings",
        tp_listings.len()
    );

    let tp_listings_map = vec_to_map(tp_listings, |x| x.id);
    let mut profitable_items: Vec<_> = profitable_item_ids
        .par_iter()
        .map(|item_id| {
            let mut tp_listings_map_clone = tp_listings_map.clone();
            let item_listings = tp_listings_map_clone
                .get_mut(item_id)
                .unwrap_or_else(|| panic!("Missing listings for item id: {}", item_id));

            item_listings.calculate_crafting_profit(
                &recipes_map,
                &items_map,
                tp_listings_map.clone(),
            )
        })
        .collect();

    profitable_items.sort_unstable_by_key(|item| item.profit);

    let header = format!(
        "{:<40} {:<15} {:<15} {:<15} {:>15} {:>15} {:>15} {:>15}",
        "Name",
        "Discipline",
        "Item id",
        "Recipe id",
        "Total profit",
        "Profit / item",
        "Items required",
        "Profit on cost"
    );
    println!("{}", header);
    println!("{}", "=".repeat(header.len()));
    for ProfitableItem {
        id: item_id,
        crafting_cost,
        profit,
        ..
    } in &profitable_items
    {
        // this can happen, presumably because of precision issues
        if crafting_cost.count == 0 {
            continue;
        }

        let name = items_map
            .get(&item_id)
            .map(|item| item.name.as_ref())
            .unwrap_or("???");

        let recipe = recipes_map.get(&item_id).expect("Missing recipe");
        println!(
            "{:<40} {:<15} {:<15} {:<15} {:>15} {:>15} {:>15} {:>15}",
            name,
            recipe
                .disciplines
                .iter()
                .map(|s| if &s[..1] == "A" {
                    // take 1st and 3rd characters to distinguish armorer/artificer
                    format!("{}{}", &s[..1], &s[2..3])
                } else {
                    s[..1].to_string()
                })
                .collect::<Vec<_>>()
                .join("/"),
            format!("i:{}", item_id),
            format!("r:{}", recipe.id),
            format!("~ {}", copper_to_string(*profit)),
            format!("{} / item", profit / crafting_cost.count),
            format!("{} items", crafting_cost.count),
            format!("{}%", (100 * profit) / crafting_cost.cost)
        );
    }

    let total_profit = profitable_items.iter().map(|item| item.profit).sum();
    println!("==========");
    println!("Total: {}", copper_to_string(total_profit));

    Ok(())
}

struct ProfitableItem {
    id: u32,
    crafting_cost: CraftingCost,
    profit: i32,
}

async fn ensure_paginated_cache<T>(
    cache_path: impl AsRef<Path>,
    url_path: &str,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    if let Ok(file) = File::open(&cache_path) {
        let stream = DeflateDecoder::new(file);
        deserialize_from(stream).map_err(|e| e.into())
    } else {
        let mut page_no = 0;
        let mut page_total = 0;

        // update page total with first request
        let mut items: Vec<T> = request_page(url_path, page_no, &mut page_total).await?;

        // fetch remaining pages in parallel batches
        page_no += 1;
        let mut request_results = stream::iter((page_no..page_total).map(|page_no| async move {
            let mut unused = 0;
            request_page::<T>(url_path, page_no, &mut unused).await
        }))
        .buffered(PARALLEL_REQUESTS)
        .collect::<Vec<Result<Vec<T>, Box<dyn std::error::Error>>>>()
        .await;

        for result in request_results.drain(..) {
            let mut new_items = result?;
            items.append(&mut new_items);
        }

        let file = File::create(cache_path)?;
        let stream = DeflateEncoder::new(file, Compression::default());
        serialize_into(stream, &items)?;

        Ok(items)
    }
}

async fn request_page<T>(
    url_path: &str,
    page_no: usize,
    page_total: &mut usize,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    let url = format!(
        "https://api.guildwars2.com/v2/{}?page={}&page_size={}",
        url_path, page_no, MAX_PAGE_SIZE
    );

    println!("Fetching {}", url);
    let response = reqwest::get(&url).await?;

    let page_total_str = response
        .headers()
        .get("X-Page-Total")
        .expect("Missing X-Page-Total header")
        .to_str()
        .expect("X-Page-Total header contains invalid string");
    *page_total = page_total_str
        .parse()
        .unwrap_or_else(|_| panic!("X-Page-Total is an invalid integer: {}", page_total_str));

    response.json::<Vec<T>>().await.map_err(|e| e.into())
}

async fn request_item_ids<T>(
    url_path: &str,
    item_ids: &Vec<u32>,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    let mut result = vec![];

    for batch in item_ids.chunks(MAX_ITEM_ID_LENGTH as usize) {
        let item_ids_str: Vec<String> = batch.iter().map(|id| id.to_string()).collect();

        let url = format!(
            "https://api.guildwars2.com/v2/{}?ids={}",
            url_path,
            item_ids_str.join(",")
        );

        println!("Fetching {}", url);
        let response = reqwest::get(&url).await?;

        result.append(&mut response.json::<Vec<T>>().await?);
    }

    Ok(result)
}

fn vec_to_map<T, F>(mut v: Vec<T>, id_fn: F) -> FxHashMap<u32, T>
where
    F: Fn(&T) -> u32,
{
    let mut map = FxHashMap::default();
    for x in v.drain(..) {
        map.insert(id_fn(&x), x);
    }
    map
}

#[derive(Debug, Copy, Clone)]
struct CraftingCost {
    cost: i32,
    count: i32,
}

// Calculate the lowest cost method to obtain the given item, using only the current high/low tp prices.
// This may involve a combination of crafting, trading and buying from vendors.
// Returns a cost and a minimum number of items that must be crafted, which may be > 1.
fn calculate_estimated_min_crafting_cost(
    item_id: u32,
    recipes_map: &FxHashMap<u32, Recipe>,
    items_map: &FxHashMap<u32, Item>,
    tp_prices_map: &FxHashMap<u32, Price>,
) -> Option<CraftingCost> {
    let item = items_map.get(&item_id);

    if let Some(item) = item {
        if item
            .flags
            .iter()
            .find(|flag| {
                *flag == "NoSell" || *flag == "AccountBound" || *flag == "SoulbindOnAcquire"
            })
            .is_some()
        {
            return None;
        }
    }

    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let crafting_cost = if let Some(recipe) = recipe {
        let mut cost = 0;
        for ingredient in &recipe.ingredients {
            let ingredient_cost = calculate_estimated_min_crafting_cost(
                ingredient.item_id,
                recipes_map,
                items_map,
                tp_prices_map,
            );

            if let Some(CraftingCost {
                cost: ingredient_cost,
                count: ingredient_cost_count,
            }) = ingredient_cost
            {
                // NB: introduces small error due to integer division
                cost += (ingredient_cost * ingredient.count) / ingredient_cost_count;
            } else {
                return None;
            }
        }

        Some(cost)
    } else {
        None
    };

    let tp_cost = tp_prices_map
        .get(&item_id)
        .filter(|price| price.sells.quantity > 0)
        .map(|price| price.sells.unit_price * output_item_count);

    let vendor_cost = item
        .and_then(|item| item.vendor_cost())
        .map(|cost| cost * output_item_count);

    if crafting_cost.is_none() && tp_cost.is_none() && vendor_cost.is_none() {
        panic!(format!("Missing cost for item id: {}", item_id));
    }

    let cost = crafting_cost
        .unwrap_or(i32::MAX)
        .min(tp_cost.unwrap_or(i32::MAX))
        .min(vendor_cost.unwrap_or(i32::MAX));

    Some(CraftingCost {
        cost,
        count: output_item_count,
    })
}

// Calculate the lowest cost method to obtain the given item, with simulated purchases from
// the trading post.
fn calculate_precise_min_crafting_cost(
    item_id: u32,
    recipes_map: &FxHashMap<u32, Recipe>,
    items_map: &FxHashMap<u32, Item>,
    tp_listings_map: &mut FxHashMap<u32, ItemListings>,
) -> Option<CraftingCost> {
    let item = items_map.get(&item_id);

    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let crafting_cost = if let Some(recipe) = recipe {
        let mut cost = 0;
        for ingredient in &recipe.ingredients {
            let ingredient_cost = calculate_precise_min_crafting_cost(
                ingredient.item_id,
                recipes_map,
                items_map,
                tp_listings_map,
            );

            if let Some(CraftingCost {
                cost: ingredient_cost,
                count: ingredient_cost_count,
            }) = ingredient_cost
            {
                // NB: introduces small error due to integer division
                cost += (ingredient_cost * ingredient.count) / ingredient_cost_count;
            } else {
                return None;
            }
        }

        Some(cost)
    } else {
        None
    };

    let tp_cost = tp_listings_map
        .get(&item_id)
        .and_then(|listings| listings.lowest_sell_offer(output_item_count));

    let vendor_cost = item
        .and_then(|item| item.vendor_cost())
        .map(|cost| cost * output_item_count);

    if crafting_cost.is_none() && tp_cost.is_none() && vendor_cost.is_none() {
        return None;
    }

    let cost = crafting_cost
        .unwrap_or(i32::MAX)
        .min(tp_cost.unwrap_or(i32::MAX))
        .min(vendor_cost.unwrap_or(i32::MAX));

    if cost == tp_cost.unwrap_or(i32::MAX) {
        tp_listings_map
            .get_mut(&item_id)
            .unwrap_or_else(|| panic!("Missing detailed prices for item id: {}", item_id))
            .buy(output_item_count);
    }

    Some(CraftingCost {
        cost,
        count: output_item_count,
    })
}

fn collect_ingredient_ids(item_id: u32, recipes_map: &FxHashMap<u32, Recipe>, ids: &mut Vec<u32>) {
    if let Some(recipe) = recipes_map.get(&item_id) {
        for ingredient in &recipe.ingredients {
            ids.push(ingredient.item_id);
            collect_ingredient_ids(ingredient.item_id, recipes_map, ids);
        }
    }
}

fn copper_to_string(copper: i32) -> String {
    let gold = copper / 100_00;
    let silver = (copper - gold * 100_00) / 100;
    let copper = copper - gold * 100_00 - silver * 100;
    format!("{}.{:02}.{:02}g", gold, silver, copper)
}

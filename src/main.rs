use bincode::{deserialize_from, serialize_into};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};

use std::fs::File;
use std::path::Path;

const MAX_PAGE_SIZE: i32 = 200; // https://wiki.guildwars2.com/wiki/API:2#Paging
const MAX_ITEM_ID_LENGTH: i32 = 200; // error returned for greater than this amount
const TRADING_POST_COMMISSION: f32 = 0.15;

const PARALLEL_REQUESTS: usize = 10;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Price {
    id: u32,
    #[serde(skip)]
    idx: usize,
    buys: PriceInfo,
    sells: PriceInfo,
}

impl Price {
    fn effective_buy_price(&self) -> i32 {
        (self.buys.unit_price as f32 * (1.0 - TRADING_POST_COMMISSION)).floor() as i32
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PriceInfo {
    unit_price: i32,
    quantity: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Recipe {
    id: u32,
    #[serde(rename = "type")]
    type_name: String,
    output_item_id: u32,
    #[serde(skip)]
    output_item_idx: usize,
    output_item_count: i32,
    time_to_craft_ms: i32,
    disciplines: Vec<String>,
    min_rating: i32,
    flags: Vec<String>,
    ingredients: Vec<RecipeIngredient>,
    chat_link: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RecipeIngredient {
    item_id: u32,
    #[serde(skip)]
    item_idx: usize,
    count: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Item {
    id: u32,
    #[serde(skip)]
    idx: usize,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ItemUpgrade {
    upgrade: String,
    item_id: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ItemListings {
    id: u32,
    #[serde(skip)]
    idx: usize,
    buys: Vec<Listing>,
    sells: Vec<Listing>,
}

impl ItemListings {
    fn calculate_crafting_profit(
        &mut self,
        recipes: &Vec<Option<Recipe>>,
        items: &Vec<Option<Item>>,
        mut tp_listings_map: Vec<Option<ItemListings>>,
    ) -> ProfitableItem {
        let mut listing_profit = 0;
        let mut total_crafting_cost = CraftingCost { cost: 0, count: 0 };

        loop {
            let crafting_cost = if let Some(crafting_cost) =
                calculate_precise_min_crafting_cost(self.idx, recipes, items, &mut tp_listings_map)
            {
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
            idx: self.idx,
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
    let mut tp_prices_source: Vec<Price> =
        ensure_paginated_cache(prices_path, "commerce/prices").await?;
    println!("Loaded {} trading post prices", tp_prices_source.len());

    println!("Loading recipes");
    let mut recipes_source: Vec<Recipe> = ensure_paginated_cache(recipes_path, "recipes").await?;
    println!("Loaded {} recipes", recipes_source.len());

    println!("Loading items");
    let mut items_source: Vec<Item> = ensure_paginated_cache(items_path, "items").await?;
    println!("Loaded {} items", items_source.len());

    // transform ids to indices for performance
    let mut id_to_idx: Vec<usize> = vec![];
    let mut idx_to_id: Vec<u32> = vec![];
    for item in &items_source {
        let id = item.id as usize;
        let idx = idx_to_id.len();

        id_to_idx.resize(id + 1, 0);
        id_to_idx[id] = idx;

        idx_to_id.push(item.id);
    }

    let mut tp_prices = vec![];
    for mut prices in tp_prices_source.drain(..) {
        let idx = id_to_idx[prices.id as usize];

        if tp_prices.len() < idx + 1 {
            tp_prices.resize(idx + 1, None)
        }
        prices.idx = idx;
        tp_prices[idx] = Some(prices);
    }

    let mut recipes = vec![];
    for mut recipe in recipes_source.drain(..) {
        let output_item_idx = id_to_idx[recipe.output_item_id as usize];

        for mut ingredient in &mut recipe.ingredients {
            ingredient.item_idx = id_to_idx[ingredient.item_id as usize];
        }

        if recipes.len() < output_item_idx + 1 {
            recipes.resize(output_item_idx + 1, None)
        }
        recipe.output_item_idx = output_item_idx;
        recipes[output_item_idx] = Some(recipe);
    }

    let mut items = vec![];
    for mut item in items_source.drain(..) {
        let idx = id_to_idx[item.id as usize];

        if items.len() < idx + 1 {
            items.resize(idx + 1, None)
        }
        item.idx = idx;
        items[idx] = Some(item);
    }

    let mut profitable_item_ids = vec![];
    let mut ingredient_ids = vec![];
    for recipe in &recipes {
        let recipe = if let Some(recipe) = recipe {
            recipe
        } else {
            continue;
        };

        if let Some(crafting_cost) = calculate_estimated_min_crafting_cost(
            recipe.output_item_idx,
            &recipes,
            &items,
            &tp_prices,
        ) {
            // some items are craftable and have no listed restrictions but are still not listable on tp
            // e.g. https://api.guildwars2.com/v2/items/39417
            if let Some(tp_prices) = &tp_prices[recipe.output_item_idx] {
                if tp_prices.effective_buy_price() * crafting_cost.count > crafting_cost.cost {
                    profitable_item_ids.push(recipe.output_item_id);
                    collect_ingredient_ids(recipe.output_item_idx, &recipes, &mut ingredient_ids);
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
    let mut tp_listings_source: Vec<ItemListings> =
        request_item_ids("commerce/listings", &request_listing_item_ids).await?;
    println!(
        "Loaded {} detailed trading post listings",
        tp_listings_source.len()
    );

    let mut tp_listings = vec![];
    for mut listings in tp_listings_source.drain(..) {
        // by default sells are listed in ascending price.
        // reverse list to allow lowest sells to be popped instead of spliced from front.
        listings.sells.reverse();

        let idx = id_to_idx[listings.id as usize];

        if tp_listings.len() < idx + 1 {
            tp_listings.resize(idx + 1, None)
        }
        listings.idx = idx;
        tp_listings[idx] = Some(listings);
    }

    let mut profitable_items: Vec<_> = profitable_item_ids
        .iter()
        .map(|item_id| {
            let tp_listings_map_clone = tp_listings.clone();
            let item_listings = tp_listings[id_to_idx[*item_id as usize]]
                .as_mut()
                .unwrap_or_else(|| panic!("Missing listings for item id: {}", item_id));

            item_listings.calculate_crafting_profit(&recipes, &items, tp_listings_map_clone)
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
        idx,
        crafting_cost,
        profit,
        ..
    } in &profitable_items
    {
        // this can happen, presumably because of precision issues
        if crafting_cost.count == 0 {
            continue;
        }

        let item = items[*idx].as_ref().expect("Missing item");
        let name = &item.name;
        let recipe = recipes[*idx].as_ref().expect("Missing recipe");

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
            format!("i:{}", item.id),
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
    idx: usize,
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

#[derive(Debug, Copy, Clone)]
struct CraftingCost {
    cost: i32,
    count: i32,
}

// Calculate the lowest cost method to obtain the given item, using only the current high/low tp prices.
// This may involve a combination of crafting, trading and buying from vendors.
// Returns a cost and a minimum number of items that must be crafted, which may be > 1.
fn calculate_estimated_min_crafting_cost(
    item_idx: usize,
    recipes: &Vec<Option<Recipe>>,
    items: &Vec<Option<Item>>,
    tp_prices: &Vec<Option<Price>>,
) -> Option<CraftingCost> {
    let item = items[item_idx].as_ref().expect("Missing item");

    if item
        .flags
        .iter()
        .find(|flag| *flag == "NoSell" || *flag == "AccountBound" || *flag == "SoulbindOnAcquire")
        .is_some()
    {
        return None;
    }

    let recipe = &recipes[item_idx];
    let output_item_count = recipe
        .as_ref()
        .map(|recipe| recipe.output_item_count)
        .unwrap_or(1);

    let crafting_cost = if let Some(recipe) = recipe {
        let mut cost = 0;
        for ingredient in &recipe.ingredients {
            let ingredient_cost = calculate_estimated_min_crafting_cost(
                ingredient.item_idx,
                recipes,
                items,
                tp_prices,
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

    let tp_cost = tp_prices[item_idx]
        .as_ref()
        .filter(|price| price.sells.quantity > 0)
        .map(|price| price.sells.unit_price * output_item_count);

    let vendor_cost = item.vendor_cost().map(|cost| cost * output_item_count);

    if crafting_cost.is_none() && tp_cost.is_none() && vendor_cost.is_none() {
        panic!(format!("Missing cost for item id: {}", item.id));
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
    item_idx: usize,
    recipes: &Vec<Option<Recipe>>,
    items: &Vec<Option<Item>>,
    tp_listings_map: &mut Vec<Option<ItemListings>>,
) -> Option<CraftingCost> {
    let item = items[item_idx].as_ref().expect("Missing item");

    let recipe = &recipes[item_idx];
    let output_item_count = recipe
        .as_ref()
        .map(|recipe| recipe.output_item_count)
        .unwrap_or(1);

    let crafting_cost = if let Some(recipe) = recipe {
        let mut cost = 0;
        for ingredient in &recipe.ingredients {
            let ingredient_cost = calculate_precise_min_crafting_cost(
                ingredient.item_idx,
                recipes,
                items,
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

    let tp_cost = tp_listings_map[item_idx]
        .as_ref()
        .and_then(|listings| listings.lowest_sell_offer(output_item_count));

    let vendor_cost = item.vendor_cost().map(|cost| cost * output_item_count);

    if crafting_cost.is_none() && tp_cost.is_none() && vendor_cost.is_none() {
        return None;
    }

    let cost = crafting_cost
        .unwrap_or(i32::MAX)
        .min(tp_cost.unwrap_or(i32::MAX))
        .min(vendor_cost.unwrap_or(i32::MAX));

    if cost == tp_cost.unwrap_or(i32::MAX) {
        tp_listings_map[item_idx]
            .as_mut()
            .unwrap_or_else(|| panic!("Missing detailed prices for item id: {}", item.id))
            .buy(output_item_count);
    }

    Some(CraftingCost {
        cost,
        count: output_item_count,
    })
}

fn collect_ingredient_ids(item_idx: usize, recipes: &Vec<Option<Recipe>>, ids: &mut Vec<u32>) {
    if let Some(recipe) = &recipes[item_idx] {
        for ingredient in &recipe.ingredients {
            ids.push(ingredient.item_id);
            collect_ingredient_ids(ingredient.item_idx, recipes, ids);
        }
    }
}

fn copper_to_string(copper: i32) -> String {
    let gold = copper / 100_00;
    let silver = (copper - gold * 100_00) / 100;
    let copper = copper - gold * 100_00 - silver * 100;
    format!("{}.{:02}.{:02}g", gold, silver, copper)
}

use bincode::{deserialize_from, serialize_into};
use colored::Colorize;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use futures::{stream, StreamExt};
use num_rational::Rational32;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use std::fs::File;
use std::path::Path;

const INCLUDE_TIMEGATED_RECIPES: bool = false; // e.g. deldrimor steel ingot
const INCLUDE_UNREFINED_MATERIALS: bool = true; // set to false to optimize for low crafting time
const INCLUDE_COMMON_ASCENDED_MATERIALS: bool = false; // i.e. pile of bloodstone dust, dragonite ore, empyreal fragment
const FILTER_DISCIPLINES: &[&str] = &[
    "Armorsmith",
    "Artificer",
    "Chef",
    "Huntsman",
    "Jeweler",
    "Leatherworker",
    "Scribe",
    "Tailor",
    "Weaponsmith",
]; // only show items craftable by these disciplines

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

impl Recipe {
    fn is_timegated(&self) -> bool {
        self.output_item_id == 46740
            || self.output_item_id == 46742
            || self.output_item_id == 46745
            || self.output_item_id == 66917
            || self.output_item_id == 66923
    }
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
        if INCLUDE_COMMON_ASCENDED_MATERIALS && self.is_common_ascended_material() {
            return Some(0);
        }

        let name = &self.name;

        if name == "Thermocatalytic Reagent"
            || name == "Spool of Jute Thread"
            || name == "Spool of Wool Thread"
            || name == "Spool of Cotton Thread"
            || name == "Spool of Linen Thread"
            || name == "Spool of Silk Thread"
            || name == "Spool of Gossamer Thread"
            || (name.ends_with("Rune of Holding") && !name.starts_with("Supreme"))
            || name == "Lump of Tin"
            || name == "Lump of Coal"
            || name == "Lump of Primordium"
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
            || name == "Crystalline Bottle"
            || name == "Bag of Mortar"
            || name == "Essence of Elegance"
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

    fn is_restricted(&self) -> bool {
        // 24749 == legacy Major Rune of the Air
        // 76363 == legacy catapult schematic
        self.id == 24749
            || self.id == 76363
            || self
                .flags
                .iter()
                .find(|flag| *flag == "AccountBound" || *flag == "SoulbindOnAcquire")
                .is_some()
    }

    fn is_common_ascended_material(&self) -> bool {
        let name = &self.name;
        name == "Empyreal Fragment" || name == "Dragonite Ore" || name == "Pile of Bloodstone Dust"
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
        mut purchased_ingredients: Option<&mut FxHashMap<u32, Rational32>>,
        limit_count: Option<i32>,
    ) -> ProfitableItem {
        let mut listing_profit = 0;
        let mut total_crafting_cost = 0;
        let mut crafting_count = 0;
        let mut total_crafting_steps = Rational32::from_integer(0);

        let mut tp_purchases = Vec::with_capacity(512);
        loop {
            if let Some(limit_count) = limit_count {
                if crafting_count >= limit_count {
                    break;
                }
            }

            tp_purchases.clear();
            let mut crafting_steps = Rational32::from_integer(0);

            let crafting_cost = if let Some(crafting_cost) = calculate_precise_min_crafting_cost(
                self.id,
                recipes_map,
                items_map,
                &tp_listings_map,
                &mut tp_purchases,
                &mut crafting_steps,
            ) {
                crafting_cost
            } else {
                break;
            };

            let buy_price = if let Some(buy_price) = self.sell() {
                buy_price
            } else {
                break;
            };

            let profit = buy_price - crafting_cost.cost;
            if profit > 0 {
                listing_profit += profit;

                total_crafting_cost += crafting_cost.cost;
                crafting_count += 1;
            } else {
                break;
            }

            for (item_id, count) in &tp_purchases {
                tp_listings_map
                    .get_mut(item_id)
                    .unwrap_or_else(|| panic!("Missing detailed prices for item id: {}", item_id))
                    .buy(count.ceil().to_integer())
                    .unwrap();
            }

            if let Some(purchased_ingredients) = &mut purchased_ingredients {
                for (item_id, count) in &tp_purchases {
                    let existing_count = purchased_ingredients
                        .entry(*item_id)
                        .or_insert(Rational32::from_integer(0));
                    *existing_count += count;
                }
            }

            total_crafting_steps += crafting_steps;
        }

        ProfitableItem {
            id: self.id,
            crafting_cost: total_crafting_cost,
            crafting_steps: total_crafting_steps,
            profit: listing_profit,
            count: crafting_count,
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

    fn lowest_sell_offer(&self, mut count: i32) -> Option<i32> {
        if !INCLUDE_UNREFINED_MATERIALS
            && (self.id == 19729
                || self.id == 19748
                || self.id == 19700
                || self.id == 19722
                || self.id == 19702
                || self.id == 19699
                || self.id == 19697
                || self.id == 89140)
        {
            return None;
        }

        let mut cost = 0;

        for listing in self.sells.iter().rev() {
            if listing.quantity < count {
                count -= listing.quantity;
                cost += listing.unit_price * listing.quantity;
            } else {
                cost += listing.unit_price * count;
                count = 0;
            }

            if count == 0 {
                break;
            }
        }

        if count > 0 {
            None
        } else {
            Some(cost)
        }
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
    let recipes_path = "recipes.bin";
    let items_path = "items.bin";

    println!("Loading recipes");
    let recipes: Vec<Recipe> = ensure_paginated_cache(recipes_path, "recipes").await?;
    println!("Loaded {} recipes", recipes.len());

    println!("Loading items");
    let items: Vec<Item> = ensure_paginated_cache(items_path, "items").await?;
    println!("Loaded {} items", items.len());

    let recipes_map = vec_to_map(recipes, |x| x.output_item_id);
    let items_map = vec_to_map(items, |x| x.id);

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let item_id: u32 = args[1].parse().expect("Failed to parse item id");
        let item = items_map.get(&item_id).expect("Item not found");

        let limit_count: Option<i32> = args.get(2).and_then(|n| n.parse().ok());

        let mut ingredient_ids = vec![];
        collect_ingredient_ids(item_id, &recipes_map, &mut ingredient_ids);

        let mut request_listing_item_ids = vec![item_id];
        request_listing_item_ids.extend(ingredient_ids);

        let tp_listings = fetch_item_listings(&request_listing_item_ids).await?;
        let tp_listings_map = vec_to_map(tp_listings, |x| x.id);

        let mut purchased_ingredients = FxHashMap::default();

        let mut tp_listings_map_clone = tp_listings_map.clone();
        let item_listings = tp_listings_map_clone
            .get_mut(&item_id)
            .unwrap_or_else(|| panic!("Missing listings for item id: {}", item_id));

        let profitable_item = item_listings.calculate_crafting_profit(
            &recipes_map,
            &items_map,
            tp_listings_map.clone(),
            Some(&mut purchased_ingredients),
            limit_count,
        );

        if profitable_item.profit == 0 {
            println!("Item is not profitable to craft");
            return Ok(());
        }

        println!(
            "Shopping list for {} x {} = {} profit ({} / step)",
            profitable_item.count,
            &item.name,
            copper_to_string(profitable_item.profit),
            profitable_item.profit_per_crafting_step()
        );
        println!("============");
        for (ingredient_id, ingredient_count) in &purchased_ingredients {
            println!(
                "{} {}",
                ingredient_count.ceil().to_integer(),
                items_map
                    .get(ingredient_id)
                    .map(|item| item.name.as_ref())
                    .unwrap_or("???")
            );
        }

        return Ok(());
    }

    println!("Loading trading post prices");
    let tp_prices: Vec<Price> = request_paginated("commerce/prices").await?;
    println!("Loaded {} trading post prices", tp_prices.len());

    let tp_prices_map = vec_to_map(tp_prices, |x| x.id);

    let mut profitable_item_ids = vec![];
    let mut ingredient_ids = vec![];
    for (item_id, recipe) in &recipes_map {
        if let Some(item) = items_map.get(item_id) {
            // we cannot sell restricted items
            if item.is_restricted() {
                continue;
            }
        }

        let mut has_discipline = false;
        for discipline in FILTER_DISCIPLINES {
            if recipe
                .disciplines
                .iter()
                .find(|s| *s == discipline)
                .is_some()
            {
                has_discipline = true;
                break;
            }
        }

        if !has_discipline {
            continue;
        }

        // some items are craftable and have no listed restrictions but are still not listable on tp
        // e.g. 39417, 79557
        // conversely, some items have a NoSell flag but are listable on the trading post
        // e.g. 66917
        let tp_prices = match tp_prices_map.get(item_id) {
            Some(tp_prices) if tp_prices.sells.quantity > 0 => tp_prices,
            _ => continue,
        };

        if let Some(crafting_cost) = calculate_estimated_min_crafting_cost(
            *item_id,
            &recipes_map,
            &items_map,
            &tp_prices_map,
        ) {
            if tp_prices.effective_buy_price() > crafting_cost.cost {
                profitable_item_ids.push(*item_id);
                collect_ingredient_ids(*item_id, &recipes_map, &mut ingredient_ids);
            }
        }
    }

    println!("Loading detailed trading post listings");
    let mut request_listing_item_ids = vec![];
    request_listing_item_ids.extend(&profitable_item_ids);
    request_listing_item_ids.extend(ingredient_ids);
    request_listing_item_ids.sort_unstable();
    request_listing_item_ids.dedup();
    let tp_listings = fetch_item_listings(&request_listing_item_ids).await?;
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
                None,
                None,
            )
        })
        .collect();

    profitable_items.sort_unstable_by_key(|item| item.profit);

    let mut line_colors = [
        colored::Color::Red,
        colored::Color::Green,
        colored::Color::Yellow,
        colored::Color::Magenta,
        colored::Color::Cyan,
    ]
    .iter()
    .cycle();

    let header = format!(
        "{:<50} {:<15} {:<15} {:<15} {:>15} {:>15} {:>15} {:>15} {:>15} {:>15}",
        "Name",
        "Discipline",
        "Item id",
        "Recipe id",
        "Total profit",
        "Items required",
        "Profit / item",
        "Crafting steps",
        "Profit / step",
        "Profit on cost",
    );
    println!("{}", header);
    println!("{}", "=".repeat(header.len()));
    for profitable_item in &profitable_items {
        // Profit may end up being 0, since potential profitable items are selected based
        // on cached prices, but the actual profit is calculated using detailed listings and
        // prices may have changed since they were cached.
        if profitable_item.count == 0 {
            continue;
        }

        let item_id = profitable_item.id;
        let name = items_map
            .get(&item_id)
            .map(|item| item.name.as_ref())
            .unwrap_or("???");

        let recipe = recipes_map.get(&item_id).expect("Missing recipe");
        let line = format!(
            "{:<50} {:<15} {:<15} {:<15} {:>15} {:>15} {:>15} {:>15} {:>15} {:>15}",
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
            format!("~ {}", copper_to_string(profitable_item.profit)),
            format!("{} items", profitable_item.count),
            format!("{} / item", profitable_item.profit_per_item()),
            format!(
                "{} steps",
                profitable_item.crafting_steps.ceil().to_integer()
            ),
            format!("{} / step", profitable_item.profit_per_crafting_step()),
            format!("{}%", profitable_item.profit_on_cost())
        );

        println!("{}", line.color(*line_colors.next().unwrap()));
    }

    let total_profit = profitable_items.iter().map(|item| item.profit).sum();
    println!("==========");
    println!("Total: {}", copper_to_string(total_profit));

    Ok(())
}

#[derive(Debug)]
struct ProfitableItem {
    id: u32,
    crafting_cost: i32,
    crafting_steps: Rational32,
    count: i32,
    profit: i32,
}

impl ProfitableItem {
    fn profit_per_item(&self) -> i32 {
        self.profit / self.count
    }

    fn profit_per_crafting_step(&self) -> i32 {
        (Rational32::from_integer(self.profit) / self.crafting_steps)
            .floor()
            .to_integer()
    }

    fn profit_on_cost(&self) -> i32 {
        (100 * self.profit) / self.crafting_cost
    }
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
        let items = request_paginated(url_path).await?;

        let file = File::create(cache_path)?;
        let stream = DeflateEncoder::new(file, Compression::default());
        serialize_into(stream, &items)?;

        Ok(items)
    }
}

async fn request_paginated<T>(url_path: &str) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
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

    Ok(items)
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

async fn fetch_item_listings(
    item_ids: &Vec<u32>,
) -> Result<Vec<ItemListings>, Box<dyn std::error::Error>> {
    let mut tp_listings: Vec<ItemListings> =
        request_item_ids("commerce/listings", &item_ids).await?;

    for listings in &mut tp_listings {
        // by default sells are listed in ascending and buys in descending price.
        // reverse lists to allow best offers to be popped instead of spliced from front.
        listings.buys.reverse();
        listings.sells.reverse();
    }

    Ok(tp_listings)
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
    source: Source,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum Source {
    Crafting,
    TradingPost,
    Vendor,
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
    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let crafting_cost = if let Some(recipe) = recipe {
        if !INCLUDE_TIMEGATED_RECIPES && recipe.is_timegated() {
            None
        } else {
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
                    ..
                }) = ingredient_cost
                {
                    cost += ingredient_cost * ingredient.count;
                } else {
                    return None;
                }
            }

            Some(div_i32_ceil(cost, output_item_count))
        }
    } else {
        None
    };

    let tp_cost = tp_prices_map
        .get(&item_id)
        .filter(|price| price.sells.quantity > 0)
        .map(|price| price.sells.unit_price);

    let vendor_cost = item.and_then(|item| item.vendor_cost()).map(|cost| cost);

    if crafting_cost.is_none() && tp_cost.is_none() && vendor_cost.is_none() {
        return None;
    }

    let cost = crafting_cost
        .unwrap_or(i32::MAX)
        .min(tp_cost.unwrap_or(i32::MAX))
        .min(vendor_cost.unwrap_or(i32::MAX));

    let source = if cost == crafting_cost.unwrap_or(i32::MAX) {
        Source::Crafting
    } else if cost == tp_cost.unwrap_or(i32::MAX) {
        Source::TradingPost
    } else {
        Source::Vendor
    };

    Some(CraftingCost { cost, source })
}

// Calculate the lowest cost method to obtain the given item, with simulated purchases from
// the trading post.
fn calculate_precise_min_crafting_cost(
    item_id: u32,
    recipes_map: &FxHashMap<u32, Recipe>,
    items_map: &FxHashMap<u32, Item>,
    tp_listings_map: &FxHashMap<u32, ItemListings>,
    tp_purchases: &mut Vec<(u32, Rational32)>,
    crafting_steps: &mut Rational32,
) -> Option<CraftingCost> {
    let item = items_map.get(&item_id);

    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let tp_purchases_ptr = tp_purchases.len();
    let crafting_steps_before = *crafting_steps;

    let crafting_cost = if let Some(recipe) = recipe {
        if !INCLUDE_TIMEGATED_RECIPES && recipe.is_timegated() {
            None
        } else {
            let mut cost = 0;
            for ingredient in &recipe.ingredients {
                let tp_purchases_ingredient_ptr = tp_purchases.len();
                let crafting_steps_before_ingredient = *crafting_steps;

                let ingredient_cost = calculate_precise_min_crafting_cost(
                    ingredient.item_id,
                    recipes_map,
                    items_map,
                    tp_listings_map,
                    tp_purchases,
                    crafting_steps,
                );

                if let Some(CraftingCost {
                    cost: ingredient_cost,
                    source,
                }) = ingredient_cost
                {
                    // NB: The trading post prices won't be completely accurate, because the reductions
                    // in liquidity for ingredients are deferred until the parent recipe is fully completed.
                    // This is to allow trading post purchases to be 'rolled back' if crafting a parent
                    // item turns out to be less profitable than buying it.
                    match source {
                        Source::TradingPost => {
                            tp_purchases.push((
                                ingredient.item_id,
                                Rational32::new(ingredient.count, output_item_count),
                            ));
                        }
                        Source::Crafting => {
                            // repeat purchases of the ingredient's children
                            for i in tp_purchases_ingredient_ptr..tp_purchases.len() {
                                let (_, count) = tp_purchases[i];
                                tp_purchases[i].1 = count * ingredient.count / output_item_count;
                            }

                            *crafting_steps = crafting_steps_before_ingredient
                                + (*crafting_steps - crafting_steps_before_ingredient)
                                    * ingredient.count
                                    / output_item_count;
                        }
                        _ => (),
                    }

                    cost += ingredient_cost * ingredient.count;
                } else {
                    return None;
                }
            }

            Some(div_i32_ceil(cost, output_item_count))
        }
    } else {
        None
    };

    let tp_cost = tp_listings_map
        .get(&item_id)
        .and_then(|listings| listings.lowest_sell_offer(1));

    let vendor_cost = item.and_then(|item| item.vendor_cost());

    if crafting_cost.is_none() && tp_cost.is_none() && vendor_cost.is_none() {
        return None;
    }

    let cost = crafting_cost
        .unwrap_or(i32::MAX)
        .min(tp_cost.unwrap_or(i32::MAX))
        .min(vendor_cost.unwrap_or(i32::MAX));

    let source = if cost == crafting_cost.unwrap_or(i32::MAX) {
        Source::Crafting
    } else if cost == tp_cost.unwrap_or(i32::MAX) {
        Source::TradingPost
    } else {
        Source::Vendor
    };

    if source != Source::Crafting {
        tp_purchases.drain(tp_purchases_ptr..);
        *crafting_steps = crafting_steps_before;
    } else {
        // increment crafting steps here, so that the final item
        // itself is also included in the crafting step count.
        *crafting_steps += Rational32::new(1, output_item_count);
    }

    Some(CraftingCost { cost, source })
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

// integer division rounding up
// see: https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
fn div_i32_ceil(x: i32, y: i32) -> i32 {
    (x + y - 1) / y
}

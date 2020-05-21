use bincode::{deserialize_from, serialize_into};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

const FILTER_DISCIPLINES: &[&str] = &["Artificer", "Tailor"];

const MAX_PAGE_SIZE: i32 = 200; // https://wiki.guildwars2.com/wiki/API:2#Paging
const TRADING_POST_COMMISSION: f32 = 0.15;

const PARALLEL_REQUESTS: usize = 10;

#[derive(Debug, Serialize, Deserialize)]
struct Price {
    id: u32,
    buys: PriceInfo,
    sells: PriceInfo,
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

#[derive(Debug, Serialize, Deserialize)]
struct ItemUpgrade {
    upgrade: String,
    item_id: i32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prices_path = "prices.bin";
    let recipes_path = "recipes.bin";
    let items_path = "items.bin";

    println!("Loading trading post prices");
    let tp_prices: Vec<Price> = ensure_paginated_cache(prices_path, "commerce/prices").await?;
    println!("Loaded {} prices", tp_prices.len());

    println!("Loading recipes");
    let recipes: Vec<Recipe> = ensure_paginated_cache(recipes_path, "recipes").await?;
    println!("Loaded {} recipes", recipes.len());

    println!("Loading items");
    let items: Vec<Item> = ensure_paginated_cache(items_path, "items").await?;
    println!("Loaded {} items", items.len());

    let tp_prices_map = paginated_cache_to_map(tp_prices, |x| x.id);
    let recipes_map = paginated_cache_to_map(recipes, |x| x.output_item_id);
    let items_map = paginated_cache_to_map(items, |x| x.id);

    let mut profitable_items = vec![];
    for (item_id, recipe) in &recipes_map {
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

        if let Some(CraftingCost { cost, count }) =
            calculate_min_crafting_cost(*item_id, &recipes_map, &tp_prices_map, &items_map)
        {
            // some items are craftable and have no listed restrictions but are still not listable on tp
            // e.g. https://api.guildwars2.com/v2/items/39417
            if let Some(tp_prices) = tp_prices_map.get(item_id) {
                let buy_price = tp_prices.buys.unit_price * count;
                let effective_buy_price =
                    (buy_price as f32 * (1.0 - TRADING_POST_COMMISSION)).floor() as i32;

                if effective_buy_price > cost {
                    profitable_items.push((item_id, cost, effective_buy_price));
                }
            }
        }
    }

    profitable_items.sort_unstable_by_key(|(_, cost, effective_buy_price)| {
        ordered_float::OrderedFloat(-effective_buy_price as f32 / *cost as f32)
    });

    for (item_id, cost, effective_buy_price) in &profitable_items {
        let name = items_map
            .get(item_id)
            .map(|item| item.name.as_ref())
            .unwrap_or("???");
        println!(
            "{:<40} i{:<10} r{:<10} {:>10}%",
            name,
            item_id,
            recipes_map
                .get(item_id)
                .map(|r| r.id)
                .expect("Missing recipe"),
            (effective_buy_price * 100 / cost) - 100
        );
    }

    Ok(())
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

fn paginated_cache_to_map<T, F>(mut v: Vec<T>, id_fn: F) -> HashMap<u32, T>
where
    F: Fn(&T) -> u32,
{
    let mut map = HashMap::new();
    for x in v.drain(..) {
        map.insert(id_fn(&x), x);
    }
    map
}

#[derive(Debug)]
struct CraftingCost {
    cost: i32,
    count: i32,
}

// Calculate the lowest cost method to obtain the given item.
// This may involve a combination of crafting, trading and buying from vendors.
// Returns a cost and a minimum number of items that must be crafted, which may be > 1.
fn calculate_min_crafting_cost(
    item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    tp_prices_map: &HashMap<u32, Price>,
    items_map: &HashMap<u32, Item>,
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
            let ingredient_cost = calculate_min_crafting_cost(
                ingredient.item_id,
                recipes_map,
                tp_prices_map,
                items_map,
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

    let vendor_cost = if item
        .filter(|item| {
            // TODO: add exceptions for all master craftsmen materials
            let name = &item.name;
            name.starts_with("Thermocatalytic")
                || (name.starts_with("Spool of")
                    && name.ends_with("Thread")
                    && !name.starts_with("Spool of Deldrimor"))
                || (name.ends_with("of Holding") && !name.starts_with("Supreme"))
                || name.starts_with("Lump of")
        })
        .is_some()
    {
        // vendor sell price is generally buy price * 8, see:
        //  https://forum-en.gw2archive.eu/forum/community/api/How-to-get-the-vendor-sell-price
        item.filter(|item| item.vendor_value > 0)
            .map(|item| item.vendor_value * 8 * output_item_count)
    } else {
        None
    };

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

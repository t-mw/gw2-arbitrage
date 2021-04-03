use colored::Colorize;
use num_rational::Rational32;
use num_traits::ToPrimitive;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Serialize, Serializer};
use structopt::StructOpt;

use std::path::PathBuf;

mod api;
mod crafting;
mod request;

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

const ITEM_STACK_SIZE: i32 = 250; // GW2 uses a "stack size" of 250

#[derive(StructOpt, Debug)]
struct Opt {
    /// Include timegated recipes such as Deldrimor Steel Ingot
    #[structopt(short = "t", long)]
    include_timegated: bool,

    /// Include recipes that require Piles of Bloodstone Dust, Dragonite Ore or Empyreal Fragments
    #[structopt(short = "a", long)]
    include_ascended: bool,

    /// If provided, output the full list of profitable recipes to this CSV file
    #[structopt(short, long, parse(from_os_str))]
    output_csv: Option<PathBuf>,

    /// If provided, print a shopping list of ingredients for the given item id
    item_id: Option<u32>,

    /// If provided, limit the maximum number of items produced for a recipe
    #[structopt(short, long)]
    count: Option<i32>,
}

#[derive(Debug, Serialize)]
struct OutputRow {
    name: String,
    disciplines: String,
    item_id: u32,
    recipe_id: u32,
    total_profit: i32,
    number_required: i32,
    profit_per_item: i32,
    crafting_steps: i32,
    profit_per_step: i32,
    #[serde(serialize_with = "serialize_rational32_to_f64")]
    profit_on_cost: Rational32,
}

fn serialize_rational32_to_f64<S>(value: &Rational32, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    const PRECISION: f64 = 1000.0;
    serializer.serialize_f64((PRECISION * value.to_f64().unwrap_or(0.0)).round() / PRECISION)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::from_args();

    let recipes_path = "recipes.bin";
    let items_path = "items.bin";

    println!("Loading recipes");
    let recipes: Vec<api::Recipe> =
        request::ensure_paginated_cache(recipes_path, "recipes").await?;
    println!("Loaded {} recipes", recipes.len());

    println!("Loading items");
    let items: Vec<api::Item> = request::ensure_paginated_cache(items_path, "items").await?;
    println!("Loaded {} items", items.len());

    let recipes_map = vec_to_map(recipes, |x| x.output_item_id);
    let items_map = vec_to_map(items, |x| x.id);

    let crafting_options = crafting::CraftingOptions {
        include_timegated: opt.include_timegated,
        include_ascended: opt.include_ascended,
        count: opt.count,
    };

    if let Some(item_id) = opt.item_id {
        let item = items_map.get(&item_id).expect("Item not found");

        let mut ingredient_ids = vec![];
        collect_ingredient_ids(item_id, &recipes_map, &mut ingredient_ids);

        let mut request_listing_item_ids = vec![item_id];
        request_listing_item_ids.extend(ingredient_ids);

        let tp_listings = request::fetch_item_listings(&request_listing_item_ids).await?;
        let tp_listings_map = vec_to_map(tp_listings, |x| x.id);

        let mut purchased_ingredients: FxHashMap<u32, Rational32> = Default::default();

        let mut tp_listings_map_clone = tp_listings_map.clone();
        let item_listings = tp_listings_map_clone
            .get_mut(&item_id)
            .unwrap_or_else(|| panic!("Missing listings for item id: {}", item_id));

        let profitable_item = item_listings.calculate_crafting_profit(
            &recipes_map,
            &items_map,
            tp_listings_map.clone(),
            Some(&mut purchased_ingredients),
            &crafting::CraftingOptions {
                include_timegated: true,
                include_ascended: true,
                ..crafting_options
            },
        );

        if profitable_item.profit == 0 {
            println!("Item is not profitable to craft");
            return Ok(());
        }

        println!("============");
        println!(
            "Shopping list for {} x {} = {} profit ({} / step)",
            profitable_item.count,
            &item.name,
            copper_to_string(profitable_item.profit),
            profitable_item.profit_per_crafting_step()
        );
        println!("============");
        for (ingredient_id, ingredient_count_ratio) in &purchased_ingredients {
            let ingredient_count = ingredient_count_ratio.ceil().to_integer();
            let ingredient_count_msg = if ingredient_count > ITEM_STACK_SIZE {
                let stack_count = ingredient_count / ITEM_STACK_SIZE;
                let remainder = ingredient_count % ITEM_STACK_SIZE;
                let remainder_msg = if remainder != 0 {
                    format!(" + {}", remainder)
                } else {
                    "".to_string()
                };
                format!(
                    "{} ({} x {}{})",
                    ingredient_count, stack_count, ITEM_STACK_SIZE, remainder_msg
                )
            } else {
                ingredient_count.to_string()
            };
            println!(
                "{} {}",
                ingredient_count_msg,
                items_map
                    .get(ingredient_id)
                    .map(|item| item.name.as_ref())
                    .unwrap_or("???")
            );
        }

        return Ok(());
    }

    println!("Loading trading post prices");
    let tp_prices: Vec<api::Price> = request::request_paginated("commerce/prices").await?;
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

        if let Some(crafting_cost) = crafting::calculate_estimated_min_crafting_cost(
            *item_id,
            &recipes_map,
            &items_map,
            &tp_prices_map,
            &crafting_options,
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
    let tp_listings = request::fetch_item_listings(&request_listing_item_ids).await?;
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
                &crafting_options,
            )
        })
        .collect();

    profitable_items.sort_unstable_by_key(|item| item.profit);

    let mut csv_writer = if let Some(path) = opt.output_csv {
        Some(csv::Writer::from_path(path)?)
    } else {
        None
    };

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
        "Disciplines",
        "Item id",
        "Recipe id",
        "Total profit",
        "No. required",
        "Profit / item",
        "Crafting steps",
        "Profit / step",
        "Profit on cost",
    );

    println!("{}", header);
    println!("{}", "=".repeat(header.len()));
    for profitable_item in &profitable_items {
        // Only required when prices are cached.
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

        let output_row = OutputRow {
            name: name.to_string(),
            disciplines: recipe
                .disciplines
                .iter()
                .map(|s| {
                    if &s[..1] == "A" {
                        // take 1st and 3rd characters to distinguish armorer/artificer
                        format!("{}{}", &s[..1], &s[2..3])
                    } else {
                        s[..1].to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("/"),
            item_id,
            recipe_id: recipe.id,
            total_profit: profitable_item.profit,
            number_required: profitable_item.count,
            profit_per_item: profitable_item.profit_per_item(),
            crafting_steps: profitable_item.crafting_steps.ceil().to_integer(),
            profit_per_step: profitable_item.profit_per_crafting_step(),
            profit_on_cost: profitable_item.profit_on_cost(),
        };

        if let Some(writer) = &mut csv_writer {
            writer.serialize(&output_row)?;
        }

        let line = format!(
            "{:<50} {:<15} {:<15} {:<15} {:>15} {:>15} {:>15} {:>15} {:>15} {:>15}",
            output_row.name,
            output_row.disciplines,
            format!("{}", output_row.item_id),
            format!("{}", output_row.recipe_id),
            format!("{}", copper_to_string(output_row.total_profit)),
            format!(
                "{} item{}",
                output_row.number_required,
                if output_row.number_required > 1 {
                    "s"
                } else {
                    ""
                }
            ),
            format!("{} / item", output_row.profit_per_item),
            format!("{} steps", output_row.crafting_steps),
            format!("{} / step", output_row.profit_per_step),
            format!(
                "{}%",
                (output_row.profit_on_cost * 100).round().to_integer()
            )
        );

        println!("{}", line.color(*line_colors.next().unwrap()));
    }

    println!("{}", "=".repeat(header.len()));
    println!("{}", header);
    println!("{}", "=".repeat(header.len()));

    let total_profit = profitable_items.iter().map(|item| item.profit).sum();
    println!("Total: {}", copper_to_string(total_profit));

    if let Some(writer) = &mut csv_writer {
        writer.flush()?;
    }

    Ok(())
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

fn collect_ingredient_ids(
    item_id: u32,
    recipes_map: &FxHashMap<u32, api::Recipe>,
    ids: &mut Vec<u32>,
) {
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

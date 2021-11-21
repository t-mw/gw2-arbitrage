use colored::Colorize;
use num_rational::Rational32;
use num_traits::ToPrimitive;
use once_cell::sync::Lazy;
use rayon::prelude::*;
use serde::{Serialize, Serializer, Deserialize};
use structopt::StructOpt;
use toml;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::fs::File;
use std::io::Read;

mod api;
mod crafting;
mod gw2efficiency;
mod request;
#[cfg(test)]
mod tests;

const VALID_DISCIPLINES: &[&str] = &[
    "Armorsmith",
    "Artificer",
    "Chef",
    "Huntsman",
    "Jeweler",
    "Leatherworker",
    "Scribe",
    "Tailor",
    "Weaponsmith",
    "Mystic Forge",
];

const ITEM_STACK_SIZE: i32 = 250; // GW2 uses a "stack size" of 250

// ignore inaccurate recipes: https://github.com/gw2efficiency/issues/issues/1532
const RECIPE_BACKLIST_IDS: &[u32] = &[
    2812,  // Minor Rune of the Air
    2825,  // Major Rune of the Air
];

#[derive(StructOpt, Debug)]
struct Opt {
    /// Include timegated recipes such as Deldrimor Steel Ingot
    #[structopt(short = "t", long)]
    include_timegated: bool,

    /// Include recipes that require Piles of Bloodstone Dust, Dragonite Ore or Empyreal Fragments
    #[structopt(short = "a", long)]
    include_ascended: bool,

    /// Output the full list of profitable recipes to this CSV file
    #[structopt(short, long, parse(from_os_str))]
    output_csv: Option<PathBuf>,

    /// Print a shopping list of ingredients for the given item id
    item_id: Option<u32>,

    /// Limit the maximum number of items produced for a recipe
    #[structopt(short, long)]
    count: Option<i32>,

    /// Only show items craftable by this discipline or comma-separated list of disciplines (e.g. -d=Weaponsmith,Armorsmith)
    #[structopt(short = "d", long = "disciplines", use_delimiter = true)]
    filter_disciplines: Option<Vec<String>>,

    /// Download recipes and items from the GW2 API, replacing any previously cached recipes and items
    #[structopt(long)]
    reset_cache: bool,

    #[structopt(long, parse(from_os_str), help = &CACHE_DIR_HELP)]
    cache_dir: Option<PathBuf>,

    #[structopt(long, parse(from_os_str), help = &CONFIG_FILE_HELP)]
    config_file: Option<PathBuf>,
}

static CACHE_DIR_HELP: Lazy<String> = Lazy::new(|| {
    format!(
        r#"Save cached recipes and items to this directory

If provided, the parent directory of the cache directory must already exist. Defaults to '{}'."#,
        cache_dir().unwrap().display()
    )
});

static CONFIG_FILE_HELP: Lazy<String> = Lazy::new(|| {
    format!(
        r#"Read config options from this file. Supported options:

    api_key = "<key-with-unlocks-scope>"

The default file location is '{}'."#,
        config_file().unwrap().display()
    )
});

#[derive(Debug, Serialize)]
struct OutputRow {
    name: String,
    disciplines: String,
    item_id: u32,
    unknown_recipes: Vec<u32>,
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

fn remove_cache_file(file: &PathBuf) -> Result<(), Box<dyn std::error::Error>>
{
    if file.exists() {
        println!("Removing existing cache file at '{}'", file.display());
        std::fs::remove_file(&file)
            .map_err(|e| format!("Failed to remove '{}' ({})", file.display(), e))?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Config {
    api_key: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::from_args();

    // API key requires scope unlocks
    let config_file = if let Some(file) = &opt.config_file {
        file.clone()
    } else {
        config_file()?
    };
    let conf: Config = if let Ok(mut file) = File::open(config_file) {
        let mut s = String::new();
        file.read_to_string(&mut s)?;
        toml::from_str(&s)?
    } else {
        Config{
            api_key: None
        }
    };
    let known_recipes = if let Some(key) = conf.api_key {
        Some(request::fetch_account_recipes(&key).await?)
    } else {
        None
    };

    let filter_disciplines = opt.filter_disciplines.as_ref().filter(|v| !v.is_empty());
    if let Some(filter_disciplines) = filter_disciplines {
        for discipline in filter_disciplines {
            if !VALID_DISCIPLINES.contains(&discipline.as_str()) {
                return Err(format!(
                    "Invalid discipline: {} (valid values are {})",
                    discipline,
                    VALID_DISCIPLINES.join(", ")
                )
                .into());
            }
        }
    }

    let cache_dir = create_cache_dir(&opt)?;
    let mut api_recipes_path = cache_dir.clone();
    api_recipes_path.push("recipes.bin");
    let mut items_path = cache_dir.clone();
    items_path.push("items.bin");
    let mut custom_recipes_path = cache_dir.clone();
    custom_recipes_path.push("custom.bin");

    if opt.reset_cache {
        remove_cache_file(&api_recipes_path)?;
        remove_cache_file(&items_path)?;
        remove_cache_file(&custom_recipes_path)?;
    }

    println!("Loading recipes");
    let api_recipes = {
        let mut api_recipes: Vec<api::Recipe> = request::ensure_paginated_cache(
            &api_recipes_path, "recipes"
        ).await?;
        for &id in RECIPE_BACKLIST_IDS {
            if let Some(pos) = api_recipes.iter().position(|r| r.id == id) {
                api_recipes.swap_remove(pos);
            }
        }
        api_recipes
    };
    println!(
        "Loaded {} recipes cached at '{}'",
        api_recipes.len(),
        api_recipes_path.display()
    );

    println!("Loading items");
    let items: Vec<api::Item> = request::ensure_paginated_cache(&items_path, "items").await?;
    println!(
        "Loaded {} items cached at '{}'",
        items.len(),
        items_path.display()
    );

    println!("Loading custom recipes");
    let custom_recipes: Vec<crafting::Recipe> = gw2efficiency::fetch_custom_recipes(&custom_recipes_path)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to fetch custom recipes: {}", e);
            vec![]
        });
    println!(
        "Loaded {} custom recipes cached at '{}'",
        custom_recipes.len(),
        custom_recipes_path.display()
    );

    let recipes: Vec<crafting::Recipe> = custom_recipes
        .into_iter()
        // prefer api recipes over custom recipes if they share the same output item id, by inserting them later
        .chain(api_recipes.into_iter().map(std::convert::From::from))
        .collect();
    let mut recipes_map = vec_to_map(recipes, |x| x.output_item_id);
    let items_map = vec_to_map(items, |x| x.id);

    let recursive_recipes = mark_recursive_recipes(&recipes_map);
    for recipe_id in recursive_recipes.into_iter() {
        recipes_map.remove(&recipe_id);
    }

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
        request_listing_item_ids.sort_unstable();
        request_listing_item_ids.dedup();

        let tp_listings = request::fetch_item_listings(&request_listing_item_ids).await?;
        let tp_listings_map = vec_to_map(tp_listings, |x| x.id);

        let mut purchased_ingredients = Default::default();
        let profitable_item = crafting::calculate_crafting_profit(
            item_id,
            &recipes_map,
            &known_recipes,
            &items_map,
            &tp_listings_map,
            Some(&mut purchased_ingredients),
            &crafting::CraftingOptions {
                include_timegated: true,
                include_ascended: true,
                ..crafting_options
            },
        );

        let profitable_item = if let Some(item) = profitable_item {
            item
        } else {
            println!("Item is not profitable to craft");
            return Ok(());
        };

        println!("============");
        println!(
            "Shopping list for {} x {} = {} profit ({} / step)",
            profitable_item.count,
            &item,
            copper_to_string(profitable_item.profit.to_integer()),
            profitable_item.profit_per_crafting_step().to_integer()
        );
        if profitable_item.max_sell != profitable_item.min_sell {
            println!(
                "Sell at: {} to {}, Breakeven price: {}",
                copper_to_string(profitable_item.max_sell),
                copper_to_string(profitable_item.min_sell),
                copper_to_string(profitable_item.breakeven),
            );
        } else {
            println!(
                "Sell at: {}, Breakeven price: {}",
                copper_to_string(profitable_item.min_sell),
                copper_to_string(profitable_item.breakeven),
            );
        }
        println!("============");
        for ((ingredient_id, ingredient_source), ingredient_count_ratio) in &purchased_ingredients {
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
                "{} {}{}",
                ingredient_count_msg,
                items_map
                    .get(ingredient_id)
                    .map_or_else(|| "???".to_string(), |item| item.to_string()),
                if *ingredient_source == crafting::Source::Vendor {
                    " (vendor)"
                } else {
                    ""
                }
            );
        }
        println!("============");
        println!(
            "Crafting steps: https://gw2efficiency.com/crafting/calculator/a~1!b~1!c~1!d~{}-{}",
            profitable_item.count, item_id
        );

        let unknown_recipes: Vec<u32> = profitable_item.used_recipes.iter().filter_map(|(id, known)| {
            if !*known {
                Some(*id)
            } else {
                None
            }
        }).collect();
        if unknown_recipes.len() > 0 {
            println!(
                "You {} craft this yet. Missing recipe id{}: {}",
                match known_recipes {
                    Some(_) => "can not",
                    None => "may not be able to",
                },
                if unknown_recipes.len() > 1 { "s" } else { "" },
                unknown_recipes.iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<String>>()
                    .join(", ")
            );
        }

        return Ok(());
    }

    println!("Loading trading post prices");
    let tp_prices: Vec<api::Price> = request::request_paginated("commerce/prices").await?;
    println!("Loaded {} trading post prices", tp_prices.len());

    let mut tp_prices_map = vec_to_map(tp_prices, |x| x.id);
    tp_prices_map.get_mut(&49429).unwrap().sells.unit_price = 6333;

    let mut profitable_item_ids = vec![];
    let mut ingredient_ids = vec![];
    for (item_id, recipe) in &recipes_map {
        if let Some(item) = items_map.get(item_id) {
            // we cannot sell restricted items
            if item.is_restricted() {
                continue;
            }
        }

        if let Some(filter_disciplines) = filter_disciplines {
            let mut has_discipline = false;
            for discipline in filter_disciplines {
                if recipe.disciplines.iter().any(|s| s == discipline) {
                    has_discipline = true;
                    break;
                }
            }

            if !has_discipline {
                continue;
            }
        }

        // some items are craftable and have no listed restrictions but are still not listable on tp
        // e.g. 39417, 79557
        // conversely, some items have a NoSell flag but are listable on the trading post
        // e.g. 66917
        let tp_prices = match tp_prices_map.get(item_id) {
            Some(tp_prices) if tp_prices.sells.quantity > 0 => tp_prices,
            _ => continue,
        };

        if let Some(crafting::EstimatedCraftingCost {
            source: crafting::Source::Crafting,
            cost: crafting_cost,
        }) = crafting::calculate_estimated_min_crafting_cost(
            *item_id,
            &recipes_map,
            &items_map,
            &tp_prices_map,
            &crafting_options,
        ) {
            if tp_prices.effective_buy_price() > crafting_cost {
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
        .filter_map(|item_id| {
            let mut ingredient_ids = vec![*item_id];
            collect_ingredient_ids(*item_id, &recipes_map, &mut ingredient_ids);

            let mut tp_listings_map_for_item: HashMap<u32, _> = HashMap::new();
            for id in ingredient_ids {
                debug_assert!(request_listing_item_ids.contains(&id));
                if let Some(listing) = tp_listings_map.get(&id).cloned() {
                    tp_listings_map_for_item.insert(id, listing);
                }
            }

            crafting::calculate_crafting_profit(
                *item_id,
                &recipes_map,
                &known_recipes,
                &items_map,
                &tp_listings_map_for_item,
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
        "Unk Recipe id",
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
            .map_or_else(|| "???".to_string(), |item| item.to_string());

        let recipe = recipes_map.get(&item_id).expect("Missing recipe");

        let output_row = OutputRow {
            name: name.to_string(),
            disciplines: recipe
                .disciplines
                .iter()
                .map(|s| {
                    if s == "Mystic Forge" {
                        return "My".to_string();
                    } else if &s[..1] == "A" {
                        // take 1st and 3rd characters to distinguish armorer/artificer
                        format!("{}{}", &s[..1], &s[2..3])
                    } else {
                        s[..1].to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("/"),
            item_id,
            unknown_recipes: profitable_item.used_recipes.iter().filter_map(|(id, known)| {
                if !*known {
                    Some(*id)
                } else {
                    None
                }
            }).collect(),
            total_profit: profitable_item.profit.to_integer(),
            number_required: profitable_item.count,
            profit_per_item: profitable_item.profit_per_item().to_integer(),
            crafting_steps: profitable_item.crafting_steps.ceil().to_integer(),
            profit_per_step: profitable_item.profit_per_crafting_step().to_integer(),
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
            format!(
                "{}",
                output_row
                    .unknown_recipes.iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<String>>()
                    .join(",")
            ),
            copper_to_string(output_row.total_profit),
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

    let total_profit: Rational32 = profitable_items.iter().map(|item| item.profit).sum();
    println!("Total: {}", copper_to_string(total_profit.to_integer()));

    if let Some(writer) = &mut csv_writer {
        writer.flush()?;
    }

    Ok(())
}

fn create_cache_dir(opt: &Opt) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = if let Some(dir) = &opt.cache_dir {
        dir.clone()
    } else {
        cache_dir()?
    };
    if !dir.exists() {
        std::fs::create_dir(&dir)
            .map_err(|e| format!("Failed to create '{}' ({})", dir.display(), e).into())
            .and(Ok(dir))
    } else {
        Ok(dir)
    }
}

fn cache_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    dirs::cache_dir()
        .filter(|d| d.exists())
        .map(|mut cache_dir| {
            cache_dir.push("gw2-arbitrage");
            cache_dir
        })
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "Failed to access current working directory".into())
}

fn config_file() -> Result<PathBuf, Box<dyn std::error::Error>> {
    dirs::config_dir()
        .filter(|d| d.exists())
        .map(|mut config_dir| {
            config_dir.push("gw2-arbitrage");
            config_dir
        })
        .or_else(|| std::env::current_dir().ok())
        .and_then(|mut path| {
            path.push("gw2-arbitrage.toml");
            Some(path)
        })
        .ok_or_else(|| "Failed to access current working directory".into())
}

fn vec_to_map<T, F>(v: Vec<T>, id_fn: F) -> HashMap<u32, T>
where
    F: Fn(&T) -> u32,
{
    let mut map = HashMap::default();
    for x in v.into_iter() {
        map.insert(id_fn(&x), x);
    }
    map
}

fn mark_recursive_recipes(recipes_map: &HashMap<u32, crafting::Recipe>) -> HashSet<u32> {
    let mut set = HashSet::new();
    for (recipe_id, recipe) in recipes_map {
        mark_recursive_recipes_internal(
            *recipe_id,
            recipe.output_item_id,
            recipes_map,
            &mut vec![],
            &mut set,
        );
    }
    set
}

fn mark_recursive_recipes_internal(
    item_id: u32,
    search_output_item_id: u32,
    recipes_map: &HashMap<u32, crafting::Recipe>,
    ingredients_stack: &mut Vec<u32>,
    set: &mut HashSet<u32>,
) {
    if set.contains(&item_id) {
        return;
    }
    if let Some(recipe) = recipes_map.get(&item_id) {
        for ingredient in &recipe.ingredients {
            if ingredient.item_id == search_output_item_id {
                set.insert(recipe.output_item_id);
                return;
            }
            // skip unnecessary recursion
            if ingredients_stack.contains(&ingredient.item_id) {
                continue;
            }
            ingredients_stack.push(ingredient.item_id);
            mark_recursive_recipes_internal(
                ingredient.item_id,
                search_output_item_id,
                recipes_map,
                ingredients_stack,
                set,
            );
            ingredients_stack.pop();
        }
    }
}

fn collect_ingredient_ids(
    item_id: u32,
    recipes_map: &HashMap<u32, crafting::Recipe>,
    ids: &mut Vec<u32>,
) {
    if let Some(recipe) = recipes_map.get(&item_id) {
        for ingredient in &recipe.ingredients {
            if ids.contains(&ingredient.item_id) {
                continue;
            }
            ids.push(ingredient.item_id);
            collect_ingredient_ids(ingredient.item_id, recipes_map, ids);
        }
    }
}

fn copper_to_string(copper: i32) -> String {
    let gold = copper / 10000;
    let silver = (copper - gold * 10000) / 100;
    let copper = copper - gold * 10000 - silver * 100;
    format!("{}.{:02}.{:02}g", gold, silver, copper)
}

use colored::Colorize;
use rayon::prelude::*;
use serde::Serialize;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use gw2_arbitrage::*;

use config::CONFIG;
use money::Money;

const ITEM_STACK_SIZE: u32 = 250; // GW2 uses a "stack size" of 250

#[derive(Debug, Serialize)]
struct OutputRow {
    name: String,
    disciplines: String,
    item_id: u32,
    unknown_recipes: Vec<u32>,
    total_profit: String,
    number_required: u32,
    profit_per_item: i32,
    crafting_steps: u32,
    profit_per_step: i32,
    profit_on_cost: f64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let known_recipes = if let Some(key) = &CONFIG.api_key {
        Some(
            request::fetch_account_recipes(&key, &CONFIG.cache_dir)
                .await
                .map_err(|e| format!("API error fetching recipe unlocks: {}", e))?,
        )
    } else {
        None
    };

    println!("Loading recipes");
    let api_recipes = {
        let mut api_recipes: Vec<api::Recipe> = request::get_data(&CONFIG.api_recipes_file, || {
            request::request_paginated("recipes", &None)
        })
        .await?;
        // If a recipe has no disciplines it cannot be crafted or discovered.
        // This appears to be used to mark deprecated recipes in the API.
        api_recipes.retain(|recipe| recipe.disciplines.len() > 0);
        api_recipes
    };
    println!(
        "Loaded {} recipes stored at '{}'",
        api_recipes.len(),
        CONFIG.api_recipes_file.display()
    );

    println!("Loading custom recipes");
    let custom_recipes: Vec<crafting::Recipe> = request::get_data(
        &CONFIG.custom_recipes_file,
        gw2efficiency::fetch_custom_recipes,
    )
    .await
    .unwrap_or_else(|e| {
        eprintln!("Failed to fetch custom recipes: {}", e);
        vec![]
    });
    println!(
        "Loaded {} custom recipes stored at '{}'",
        custom_recipes.len(),
        CONFIG.custom_recipes_file.display()
    );

    println!("Loading items");
    let items: Vec<api::Item> = request::get_data(&CONFIG.items_file, || async {
        let api_items: Vec<api::ApiItem> =
            request::request_paginated("items", &CONFIG.lang).await?;
        Ok(api_items
            .into_iter()
            .map(|api_item| api::Item::from(api_item))
            .collect())
    })
    .await?;
    println!(
        "Loaded {} items stored at '{}'",
        items.len(),
        CONFIG.items_file.display()
    );

    let recipes: Vec<crafting::Recipe> = custom_recipes
        .into_iter()
        // prefer api recipes over custom recipes if they share the same output item id, by inserting them later
        .chain(api_recipes.into_iter().map(std::convert::From::from))
        .filter(|recipe| {
            if let Some(recipe_blacklist) = &CONFIG.recipe_blacklist {
                if let Some(id) = recipe.id {
                    if recipe_blacklist.contains(&id) {
                        return false;
                    }
                }
            }
            if let Some(item_blacklist) = &CONFIG.item_blacklist {
                for ingredient in &recipe.ingredients {
                    if item_blacklist.contains(&ingredient.item_id) {
                        return false;
                    }
                }
            }

            true
        })
        .collect();
    let mut recipes_map = vec_to_map(recipes, |x| x.output_item_id);
    let items_map = vec_to_map(items, |x| x.id);

    let recursive_recipes = mark_recursive_recipes(&recipes_map);
    for recipe_id in recursive_recipes.into_iter() {
        recipes_map.remove(&recipe_id);
    }

    if let Some(item_id) = CONFIG.item_id {
        let item = items_map.get(&item_id).expect("Item not found");

        let mut ingredient_ids = vec![];
        collect_ingredient_ids(item_id, &recipes_map, &mut ingredient_ids);

        let mut request_listing_item_ids = vec![item_id];
        request_listing_item_ids.extend(ingredient_ids);
        request_listing_item_ids.sort_unstable();
        request_listing_item_ids.dedup();

        let tp_listings =
            request::fetch_item_listings(&request_listing_item_ids, Some(&CONFIG.cache_dir))
                .await?;
        let tp_listings_map = vec_to_map(tp_listings, |x| x.id);

        let mut purchased_ingredients = Default::default();
        let profitable_item = crafting::calculate_crafting_profit(
            item_id,
            &recipes_map,
            &items_map,
            &tp_listings_map,
            Some(&mut purchased_ingredients),
            &config::CraftingOptions {
                include_timegated: true,
                ..CONFIG.crafting
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
            "Shopping list for {} x {} = {} profit ({} / step, {}%)",
            profitable_item.count,
            &item,
            Money::from_copper(profitable_item.profit.to_copper_value()),
            profitable_item.profit_per_crafting_step().to_copper_value(),
            (profitable_item.profit_on_cost() * 100_f64)
                .round(),
        );
        let price_msg = if profitable_item.max_sell == profitable_item.min_sell {
            format!("{}", profitable_item.min_sell)
        } else {
            format!(
                "{} to {}",
                profitable_item.max_sell,
                profitable_item.min_sell,
            )
        };
        println!(
            "Sell at: {}, Money Required: {}, Breakeven price: {}",
            price_msg,
            profitable_item.crafting_cost.increase_by_listing_fee(),
            profitable_item.breakeven,
        );

        println!("============");
        let mut sorted_ingredients: Vec<(
            &(u32, crafting::Source),
            &crafting::PurchasedIngredient,
        )> = purchased_ingredients.iter().collect();
        sorted_ingredients.sort_unstable_by(|a, b| {
            if b.0 .1 == a.0 .1 {
                match b.1.count.cmp(&a.1.count) {
                    Ordering::Equal => match b.1.total_cost.cmp(&a.1.total_cost) {
                        Ordering::Equal => b.0 .0.cmp(&a.0 .0),
                        v => v,
                    },
                    v => v,
                }
            } else if b.0 .1 == crafting::Source::Vendor {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        });
        let mut inventory = 0;
        for ((ingredient_id, ingredient_source), ingredient) in sorted_ingredients {
            let purchase_count = if *ingredient_source == crafting::Source::Vendor {
                items_map
                    .get(ingredient_id)
                    .unwrap_or_else(|| panic!("Missing item for ingredient {}", ingredient_id))
                    .vendor_cost()
                    .unwrap_or((Money::from_copper(0), 1))
                    .1
            } else {
                ITEM_STACK_SIZE
            };
            let ingredient_count_msg = if purchase_count > 1 && ingredient.count > purchase_count {
                let stack_count = ingredient.count / purchase_count;
                inventory += ingredient.count.div_ceil(ITEM_STACK_SIZE);
                let remainder = ingredient.count % purchase_count;
                let remainder_msg = if remainder != 0 {
                    format!(" + {}", remainder)
                } else {
                    "".to_string()
                };
                format!(
                    "{} ({} x {}{})",
                    ingredient.count, stack_count, purchase_count, remainder_msg
                )
            } else {
                inventory += 1;
                ingredient.count.to_string()
            };
            let source_msg = match *ingredient_source {
                crafting::Source::TradingPost => {
                    if ingredient.max_price == ingredient.min_price {
                        format!(
                            " (at {}) Subtotal: {}",
                            ingredient.min_price,
                            ingredient.total_cost,
                        )
                    } else {
                        format!(
                            " (at {} to {}) Subtotal: {}",
                            ingredient.min_price,
                            ingredient.max_price,
                            ingredient.total_cost,
                        )
                    }
                }
                crafting::Source::Vendor => {
                    let vendor_cost = items_map
                        .get(ingredient_id)
                        .unwrap_or_else(|| panic!("Missing item for ingredient {}", ingredient_id))
                        .vendor_cost();
                    if let Some((cost, purchase_count)) = vendor_cost {
                        if purchase_count > 1 {
                            format!(
                                " (vendor: {} per {}) Subtotal: {}",
                                cost * purchase_count,
                                purchase_count,
                                cost * ingredient.count,
                            )
                        } else {
                            format!(
                                " (vendor: {}) Subtotal: {}",
                                cost,
                                cost * ingredient.count,
                            )
                        }
                    } else {
                        "".to_string()
                    }
                }
                crafting::Source::Crafting => "".to_string(),
            };
            println!(
                "{} {}{}",
                ingredient_count_msg,
                items_map
                    .get(ingredient_id)
                    .map_or_else(|| "???".to_string(), |item| item.to_string()),
                source_msg,
            );
        }

        println!("============");
        println!("Max inventory slots: {}", inventory + 1); // + 1 for the crafting output
        println!(
            "Crafting steps: https://gw2efficiency.com/crafting/calculator/a~1!b~1!c~1!d~{}-{}",
            profitable_item.count, item_id
        );
        for (item_id, count, recipe) in profitable_item.crafted_items.sorted(item_id, &recipes_map) {
            let num_crafted = count / recipe.output_item_count;
            let item_name = items_map
                .get(&item_id)
                .map_or_else(|| "???".to_string(), |item| item.to_string());
            // TODO: we want _these_ well ordered as well...
            let ingredients = recipe.ingredients.iter().map(|ingredient| {
                let ingredient_name = items_map
                    .get(&ingredient.item_id)
                    .map_or_else(|| "???".to_string(), |item| item.to_string());
                format!("{} {}", ingredient.count * num_crafted, ingredient_name)
            }).collect::<Vec<String>>().join(" ");
            if recipe.output_item_count > 1 {
                println!("{} (makes {}) {} from {}", num_crafted, count, item_name, ingredients);
            } else {
                println!("{} {} from {}", count, item_name, ingredients);
            }
        }

        let unknown_recipes: Vec<u32> = profitable_item
            .crafted_items
            .unknown_recipes(&recipes_map, &known_recipes)
            .iter()
            .map(|&id| id)
            .collect();
        if unknown_recipes.len() > 0 {
            let req_recipes = unknown_recipes
                .iter()
                .map(|id| {
                    let recipe_names = items_map
                        .iter()
                        .filter(|(_, item)| {
                            if let Some(unlocks) = &item.recipe_unlocks() {
                                unlocks.iter().filter(|&recipe_id| id == recipe_id).count() > 0
                            } else {
                                false
                            }
                        })
                        .map(|(_, item)| format!("{}", &item.name))
                        .collect::<Vec<String>>()
                        .join(" or ");
                    if recipe_names.len() > 0 {
                        recipe_names
                    } else {
                        // recipe 5424 for item 29407 has no unlock item, possibly others
                        format!("Recipe {} is not available!", &id)
                    }
                })
                .collect::<Vec<String>>()
                .join("\n");
            println!(
                "You {} craft this yet. Required recipes{}:\n{}",
                match known_recipes {
                    Some(_) => "can not",
                    None => "may not be able to",
                },
                if unknown_recipes.len() > 1 { "s" } else { "" },
                req_recipes,
            );
        }

        let leftovers = profitable_item.crafted_items.leftovers;
        if !leftovers.is_empty() {
            println!("Leftovers:");
            for (leftover_id, (count, cost, _)) in leftovers.iter() {
                println!(
                    "{} {}, breakeven: {} each",
                    count,
                    items_map
                        .get(&leftover_id)
                        .map_or_else(|| "???".to_string(), |item| item.to_string()),
                    cost.trading_post_listing_price(),
                );
            }
        }

        return Ok(());
    }

    println!("Loading trading post prices");
    let tp_prices: Vec<api::Price> = request::request_paginated("commerce/prices", &None).await?;
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

        if let Some(filter_disciplines) = &CONFIG.filter_disciplines {
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
            &CONFIG.crafting,
        ) {
            let effective_buy_price = Money::from_copper(tp_prices.buys.unit_price as i32)
                .trading_post_sale_revenue();
            if effective_buy_price > crafting_cost {
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
    // Caching these is pointless, as the vector changes on each run, leading to new URLs
    let tp_listings = request::fetch_item_listings(&request_listing_item_ids, None).await?;
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
                &items_map,
                &tp_listings_map_for_item,
                None,
                &CONFIG.crafting,
            )
        })
        .collect();

    profitable_items.sort_unstable_by_key(|item| item.profit);

    let mut csv_writer = if let Some(path) = &CONFIG.output_csv {
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
        "{:<50} {:<15} {:<15} {:<20} {:>15} {:>15} {:>15} {:>15} {:>15} {:>15}",
        "Name",
        "Disciplines",
        "Item id",
        "Req. Recipe Ids",
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
                .map(|d| {
                    let s = d.to_string();
                    match &s[..1] {
                        // take 1st and 8th characters to distinguish Merchant/Mystic Forge
                        "M" => format!("{}{}", &s[..1], &s[7..8]),
                        // take 1st and 3rd characters to distinguish Scribe/Salvage and
                        // Armorsmith/Artificer/Achievement
                        "A" | "S" => format!("{}{}", &s[..1], &s[2..3]),
                        // take 1st and 4th characters to distinguish Chef/Charge
                        "C" => format!("{}{}", &s[..1], &s[3..4]),
                        l => l.to_string(),
                    }
                })
                .collect::<Vec<_>>()
                .join("/"),
            item_id,
            unknown_recipes: profitable_item
                .crafted_items
                .unknown_recipes(&recipes_map, &known_recipes)
                .iter()
                .map(|&id| id)
                .collect(),
            total_profit: profitable_item.profit.to_string(),
            number_required: profitable_item.count,
            profit_per_item: profitable_item.profit_per_item().to_copper_value(),
            crafting_steps: profitable_item.crafting_steps,
            profit_per_step: profitable_item.profit_per_crafting_step().to_copper_value(),
            profit_on_cost: profitable_item.profit_on_cost(),
        };

        if let Some(writer) = &mut csv_writer {
            writer.serialize(&output_row)?;
        }

        let line = format!(
            "{:<50} {:<15} {:<15} {:<20} {:>15} {:>15} {:>15} {:>15} {:>15} {:>15}",
            output_row.name,
            output_row.disciplines,
            format!("{}", output_row.item_id),
            format!(
                "{}",
                output_row
                    .unknown_recipes
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<String>>()
                    .join(",")
            ),
            output_row.total_profit,
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
            format!("{}%", (output_row.profit_on_cost * 100_f64).round())
        );

        println!("{}", line.color(*line_colors.next().unwrap()));
    }

    println!("{}", "=".repeat(header.len()));
    println!("{}", header);
    println!("{}", "=".repeat(header.len()));

    let total_profit: Money = profitable_items.iter().map(|item| item.profit).sum();
    println!("Total: {}", total_profit);

    if let Some(writer) = &mut csv_writer {
        writer.flush()?;
    }

    Ok(())
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

trait DivCeil {
    fn div_ceil(&self, other: Self) -> Self;
}
impl DivCeil for u32 {
    fn div_ceil(&self, other: Self) -> Self {
        (self + other - 1) / other
    }
}

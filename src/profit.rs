use rayon::prelude::*;

use std::collections::{HashMap, HashSet};

use crate::request;
use crate::recipe::Recipe;
use crate::crafting;
use crate::money::Money;
use crate::item::Item;
use crate::config;
use config::CONFIG;
use crate::api;

/// Return a items which are profitable to make at least one of, and their ingredients, for further
/// scrutiny
pub fn find_profitable_items(
    tp_prices_map: &HashMap<u32, api::Price>,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
) -> (Vec<u32>, Vec<u32>) {
    let mut profitable_item_ids = vec![];
    let mut ingredient_ids = vec![];
    for (item_id, recipe) in recipes_map {
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
                if let Some(recipe) = recipes_map.get(&item_id) {
                    recipe.collect_ingredient_ids(&recipes_map, &mut ingredient_ids);
                }
            }
        }
    }

    (profitable_item_ids, ingredient_ids)
}

/// Compute exact profit of profitable items independently in parallel
pub fn profitable_item_list(
    tp_listings_map: &HashMap<u32, api::ItemListings>,
    profitable_item_ids: &Vec<u32>,
    request_listing_item_ids: &Vec<u32>,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
) -> Vec<crafting::ProfitableItem> {
    let mut profitable_items: Vec<_> = profitable_item_ids
        .par_iter()
        .filter_map(|item_id| {
            let mut ingredient_ids = vec![*item_id];
            if let Some(recipe) = recipes_map.get(&item_id) {
                recipe.collect_ingredient_ids(&recipes_map, &mut ingredient_ids);
            }

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

    profitable_items
}

pub async fn calc_item_profit(
    item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
    known_recipes: &Option<HashSet<u32>>,
    notify: Option<&dyn Fn(&str)>,
) -> Result<(
    Option<crafting::ProfitableItem>,
    HashMap<(u32, crafting::Source), crafting::PurchasedIngredient>,
    Vec<u32>,
    HashMap<u32, api::Price>,
), Box<dyn std::error::Error>> {
    let mut items_to_price = vec![];

    let mut unknown_recipes = HashSet::new();
    let mut recipe_prices = Default::default();
    if let Some(recipe) = recipes_map.get(&item_id) {
        recipe.collect_ingredient_ids(&recipes_map, &mut items_to_price);

        recipe.collect_unknown_recipe_ids(&recipes_map, &known_recipes, &mut unknown_recipes);
        let recipe_items: Vec<u32> = items_map
            .iter()
            .filter_map(|(_, item)| {
                if let Some(unlocks) = &item.recipe_unlocks() {
                    if unlocks.iter().filter(|&recipe_id| unknown_recipes.contains(recipe_id)).count() > 0 {
                        return Some(item.id);
                    }
                }
                None
            })
            .collect();
        let prices: Vec<api::Price> = request::request_item_ids("commerce/prices", &recipe_items, None, notify)
            .await
            .unwrap_or(Default::default()); // ignore "all ids provided are invalid" (and all other errors)
        recipe_prices = vec_to_map(prices, |x| x.id);
    }

    let mut request_listing_item_ids = vec![item_id];
    request_listing_item_ids.extend(items_to_price);
    request_listing_item_ids.sort_unstable();
    request_listing_item_ids.dedup();

    let tp_listings =
        request::fetch_item_listings(&request_listing_item_ids, Some(&CONFIG.cache_dir), notify)
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

    let required_unknown_recipes: Vec<u32> = if let Some(profitable_item) = &profitable_item {
        profitable_item
        .crafted_items
        .crafted
        .keys()
        .filter_map(|item_id| {
            if let Some(recipe) = recipes_map.get(&item_id) {
                if let Some(recipe_id) = recipe.id {
                    if unknown_recipes.contains(&recipe_id) {
                        return Some(recipe_id)
                    }
                }
            }
            None
        })
        .collect()
    } else {
        Default::default()
    };

    Ok((profitable_item, purchased_ingredients, required_unknown_recipes, recipe_prices))
}

pub fn vec_to_map<T, F>(v: Vec<T>, id_fn: F) -> HashMap<u32, T>
where
    F: Fn(&T) -> u32,
{
    let mut map = HashMap::default();
    for x in v.into_iter() {
        map.insert(id_fn(&x), x);
    }
    map
}

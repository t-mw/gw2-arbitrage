use rayon::prelude::*;

use std::collections::{BTreeMap, HashMap, HashSet};
use num_traits::Zero;

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
) -> Vec<ProfitableItem> {
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

            calculate_crafting_profit(
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
    Option<ProfitableItem>,
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
    let profitable_item = calculate_crafting_profit(
        item_id,
        &recipes_map,
        &items_map,
        &tp_listings_map,
        Some(&mut purchased_ingredients),
        &CONFIG.crafting,
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

pub fn calculate_crafting_profit(
    item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
    tp_listings_map: &HashMap<u32, api::ItemListings>,
    mut purchased_ingredients: Option<&mut HashMap<(u32, crafting::Source), crafting::PurchasedIngredient>>,
    opt: &config::CraftingOptions,
) -> Option<ProfitableItem> {
    let mut tp_listings_map: BTreeMap<u32, ItemListings> = tp_listings_map
        .clone()
        .into_iter()
        .map(|(id, listings)| (id, ItemListings::from(listings)))
        .collect();

    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);
    let threshold = Money::from_copper(opt.threshold.unwrap_or(0) as i32);

    let mut listing_profit = Money::zero();
    let mut total_crafting_cost = Money::zero();
    let mut crafting_count = 0;
    let mut crafted_items = crafting::CraftedItems::default();

    let mut min_sell = 0;
    let max_sell = tp_listings_map
        .get(&item_id)
        .map_or_else(|| opt.threshold.unwrap_or(0), |listings| {
            listings
                .buys
                .last()
                .map_or(0, |l| l.unit_price)
        });
    let mut breakeven = Money::zero();

    // simulate crafting 1 item per loop iteration until it becomes unprofitable
    loop {
        if let Some(count) = opt.count {
            if crafting_count + output_item_count > count {
                break;
            }
        }

        let mut context = crafting::PreciseCraftingCostContext {
            purchases: vec![],
            items: crafted_items.clone(),
        };

        let crafting_cost = if let Some(crafting::PreciseCraftingCost {
            source: crafting::Source::Crafting,
            cost,
        }) = crafting::calculate_precise_min_crafting_cost(
            item_id,
            output_item_count,
            recipes_map,
            items_map,
            &mut tp_listings_map,
            &mut context,
            opt,
        ) {
            cost
        } else {
            break;
        };


        let (buy_price, min_buy) = if let Some(price) = opt.value {
            (Money::from_copper(price as i32) * output_item_count, price)
        } else if let Some((buy_price, min_buy)) = tp_listings_map
            .get_mut(&item_id)
            .unwrap_or_else(|| panic!("Missing listings for item id: {}", item_id))
            .sell(output_item_count)
        {
            (buy_price, min_buy)
        } else {
            break;
        };

        // Ensure buy_price is larger before subtracting cost for profit
        if buy_price < crafting_cost + threshold {
            break;
        }

        listing_profit += buy_price - crafting_cost;
        total_crafting_cost += crafting_cost;
        crafting_count += output_item_count;
        crafted_items = context.items;

        min_sell = min_buy;
        // Breakeven is based on the last/most expensive to craft
        breakeven = crafting_cost / output_item_count;

        // Finalize purchases
        for (purchase_id, count, purchase_source) in &context.purchases {
            let (cost, min_sell, max_sell) = if let crafting::Source::TradingPost = *purchase_source {
                let listing = tp_listings_map.get_mut(purchase_id).unwrap_or_else(|| {
                    panic!(
                        "Missing listings for ingredient {} of item id {}",
                        purchase_id, item_id
                    )
                });
                listing.pending_buy_quantity -= *count;
                let (cost, min_sell, max_sell) = listing.buy(*count).unwrap_or_else(|| {
                    panic!(
                        "Expected to be able to buy {} of ingredient {} for item id {}",
                        count, purchase_id, item_id
                    )
                });
                (cost, min_sell, max_sell)
            } else {
                (0, 0, 0)
            };

            if let Some(purchased_ingredients) = &mut purchased_ingredients {
                let ingredient = purchased_ingredients
                    .entry((*purchase_id, *purchase_source))
                    .or_insert_with(|| crafting::PurchasedIngredient {
                        count: 0,
                        max_price: Money::default(),
                        min_price: Money::default(),
                        total_cost: Money::default(),
                    });
                ingredient.count += count;
                if ingredient.min_price.is_zero() {
                    ingredient.min_price = Money::from_copper(min_sell as i32);
                }
                ingredient.max_price = Money::from_copper(max_sell as i32);
                ingredient.total_cost += Money::from_copper(cost as i32);
            }
        }
        debug_assert!(tp_listings_map
            .iter()
            .all(|(_, listing)| listing.pending_buy_quantity == 0));

    }

    if crafting_count > 0 && !listing_profit.is_zero() {
        Some(ProfitableItem {
            id: item_id,
            crafting_cost: total_crafting_cost,
            profit: listing_profit,
            count: crafting_count,
            max_sell: Money::from_copper(max_sell as i32),
            min_sell: Money::from_copper(min_sell as i32),
            breakeven: breakeven.trading_post_listing_price(),
            crafting_steps: crafted_items.crafting_steps(recipes_map).to_integer(),
            crafted_items,
        })
    } else {
        None
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProfitableItem {
    pub id: u32,
    pub crafting_cost: Money,
    pub count: u32,
    pub profit: Money,
    pub max_sell: Money,
    pub min_sell: Money,
    pub breakeven: Money,
    pub crafting_steps: u32,
    pub crafted_items: crafting::CraftedItems,
}

impl ProfitableItem {
    pub fn profit_per_item(&self) -> Money {
        self.profit / self.count
    }

    pub fn profit_per_crafting_step(&self) -> Money {
        self.profit / self.crafting_steps
    }

    pub fn profit_on_cost(&self) -> f64 {
        self.profit.percent(self.crafting_cost)
    }
}

#[derive(Clone, Debug)]
pub struct ItemListings {
    pub id: u32,
    pub buys: Vec<Listing>,
    pub sells: Vec<Listing>,
    pub pending_buy_quantity: u32,
}

#[derive(Clone, Debug)]
pub struct Listing {
    pub unit_price: u32,
    pub quantity: u32,
}

impl ItemListings {
    fn buy(&mut self, mut count: u32) -> Option<(u32, u32, u32)> {
        let mut cost = 0;
        let mut min_sell = 0;
        let mut max_sell = 0;

        while count > 0 {
            // sells are sorted in descending price
            let remove = if let Some(listing) = self.sells.last_mut() {
                listing.quantity -= 1;
                count -= 1;
                if min_sell == 0 {
                    min_sell = listing.unit_price;
                }
                max_sell = listing.unit_price;
                cost += listing.unit_price;
                listing.quantity.is_zero()
            } else {
                return None;
            };

            if remove {
                self.sells.pop();
            }
        }

        Some((cost, min_sell, max_sell))
    }

    fn sell(&mut self, mut count: u32) -> Option<(Money, u32)> {
        let mut revenue = Money::zero();
        let mut min_buy = 0;

        while count > 0 {
            // buys are sorted in ascending price
            let remove = if let Some(listing) = self.buys.last_mut() {
                listing.quantity -= 1;
                count -= 1;
                min_buy = listing.unit_price;
                revenue += Money::from_copper(listing.unit_price as i32).trading_post_sale_revenue();
                listing.quantity.is_zero()
            } else {
                return None;
            };

            if remove {
                self.buys.pop();
            }
        }

        Some((revenue, min_buy))
    }

    pub fn lowest_sell_offer(&self, mut quantity: u32) -> Option<u32> {
        debug_assert!(!quantity.is_zero());

        let mut cost = 0;
        let mut pending_buy_quantity = self.pending_buy_quantity;

        for listing in self.sells.iter().rev() {
            let mut remaining_listing_quantity = listing.quantity;
            if pending_buy_quantity > 0 {
                if pending_buy_quantity >= remaining_listing_quantity {
                    pending_buy_quantity -= remaining_listing_quantity;
                    remaining_listing_quantity = 0;
                } else {
                    remaining_listing_quantity -= pending_buy_quantity;
                    pending_buy_quantity = 0;
                }
            }

            if remaining_listing_quantity > 0 {
                if remaining_listing_quantity < quantity {
                    quantity -= remaining_listing_quantity;
                    cost += remaining_listing_quantity * listing.unit_price;
                } else {
                    cost += quantity * listing.unit_price;
                    quantity = 0;
                }
            }

            if quantity.is_zero() {
                break;
            }
        }

        if quantity > 0 {
            None
        } else {
            Some(cost)
        }
    }
}

impl From<api::ItemListings> for ItemListings {
    fn from(v: api::ItemListings) -> Self {
        ItemListings {
            id: v.id,
            buys: v
                .buys
                .into_iter()
                .map(|listing| Listing {
                    unit_price: listing.unit_price,
                    quantity: listing.quantity.into(),
                })
                .collect(),
            sells: v
                .sells
                .into_iter()
                .map(|listing| Listing {
                    unit_price: listing.unit_price,
                    quantity: listing.quantity.into(),
                })
                .collect(),
            pending_buy_quantity: 0,
        }
    }
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

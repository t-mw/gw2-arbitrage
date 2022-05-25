use crate::api;
use crate::config;
use crate::recipe::Recipe;
use crate::item::Item;
use crate::money::Money;
use crate::profit;

use num_rational::Ratio;
use num_traits::Zero;

use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Copy, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Source {
    Crafting,
    TradingPost,
    Vendor,
}

#[derive(Debug, Copy, Clone)]
pub struct EstimatedCraftingCost {
    pub cost: Money,
    pub source: Source,
}

// Calculate the lowest cost method to obtain the given item, using only the current high/low tp prices.
// This may involve a combination of crafting, trading and buying from vendors.
pub fn calculate_estimated_min_crafting_cost(
    item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
    tp_prices_map: &HashMap<u32, api::Price>,
    opt: &config::CraftingOptions,
) -> Option<EstimatedCraftingCost> {
    let item = items_map.get(&item_id);
    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let crafting_cost = recipe.and_then(|recipe| {
        if !opt.include_timegated && recipe.is_timegated() {
            None
        } else {
            let mut cost = Money::zero();
            for ingredient in &recipe.ingredients {
                let ingredient_cost = calculate_estimated_min_crafting_cost(
                    ingredient.item_id,
                    recipes_map,
                    items_map,
                    tp_prices_map,
                    opt,
                );

                if let Some(EstimatedCraftingCost {
                    cost: ingredient_cost,
                    ..
                }) = ingredient_cost
                {
                    cost += ingredient_cost * ingredient.count;
                } else {
                    return None;
                }
            }

            Some(cost / output_item_count)
        }
    });

    let tp_cost = tp_prices_map
        .get(&item_id)
        .filter(|price| price.sells.quantity > 0)
        .map(|price| Money::from_copper(price.sells.unit_price as i32));

    let vendor_cost = item.and_then(|item| {
        item.vendor_cost().map_or_else(|| item.token_value(), |cost| Some(cost.0))
    });
    let cost = tp_cost.inner_min(crafting_cost).inner_min(vendor_cost)?;

    // give trading post precedence over crafting if costs are equal
    let source = if tp_cost == Some(cost) {
        Source::TradingPost
    } else if crafting_cost == Some(cost) {
        Source::Crafting
    } else {
        Source::Vendor
    };

    Some(EstimatedCraftingCost { cost, source })
}

// Exact

#[derive(Debug, Copy, Clone)]
pub struct PreciseCraftingCost {
    pub cost: Money,
    pub source: Source,
}

pub struct PreciseCraftingCostContext {
    pub purchases: Vec<(u32, u32, Source)>, // id, count, Source
    pub items: CraftedItems,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct CraftedItems {
    pub crafted: HashMap<u32, u32>, // id, count
    pub leftovers: HashMap<u32, (u32, Money, Source)>,
}

impl CraftedItems {
    pub fn crafting_steps(
        &self,
        recipes_map: &HashMap<u32, Recipe>,
    ) -> Ratio<u32> {
        let total_crafting_steps = self.crafted.iter().map(|(item_id, &count)| {
            let recipe = recipes_map.get(item_id);
            let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);
            Ratio::new(count, output_item_count)
        }).reduce(|total, count| total + count).unwrap();
        debug_assert!(total_crafting_steps.is_integer());
        total_crafting_steps
    }

    // TODO: merge w/recipes? The difference is there we need all regardless of what will be
    // crafted; here we know what will be crafted.
    pub fn unknown_recipes(
        &self,
        recipes_map: &HashMap<u32, Recipe>,
        known_recipes: &Option<HashSet<u32>>,
    ) -> HashSet<u32> {
        let mut unknown_recipes = HashSet::new();
        for item_id in self.crafted.keys() {
            let recipe = if let Some(recipe) = recipes_map.get(item_id) {
                recipe
            } else {
                continue;
            };
            if let Some(id) = recipe.id.filter(|id| !unknown_recipes.contains(id)) {
                if !recipe.is_automatic() {
                    // If we have no known recipes, assume we know none
                    if known_recipes
                        .as_ref()
                            .filter(|recipes| recipes.contains(&id))
                            .is_none()
                    {
                        unknown_recipes.insert(id);
                    }
                }
            }
        }
        unknown_recipes
    }

    /// Sort in the order which will remove ingredients from the inventory fastest
    fn sort_ingredients<'a>(
        &self,
        item_id: u32,
        count: u32,
        recipes_map: &'a HashMap<u32, Recipe>
    ) -> (u32, Vec<(u32, u32, &'a Recipe)>) {
        let mut ingredients = Vec::new();
        let recipe = recipes_map.get(&item_id).unwrap();
        for ingredient in &recipe.ingredients {
            let crafted = self.crafted.get(&ingredient.item_id).unwrap_or(&0);
            if *crafted <= 0 {
                continue;
            }
            // Let each recipe which uses something get full credit for it; then
            // only add it once to the crafting list once weights are determined
            ingredients.push((
                ingredient.item_id, *crafted,
                self.sort_ingredients(ingredient.item_id, *crafted, &recipes_map),
            ));
        }

        // Some MF recipes have the same item/count repeated
        ingredients.sort_by(|a, b| {
            if a.1 == b.1 {
                b.0.cmp(&a.0)
            } else {
                b.1.cmp(&a.1)
            }
        });

        let mut sorted = Vec::new();
        let mut sum = 0;
        let mut used = HashSet::new();
        for ingredient in ingredients.iter() {
            sum += ingredient.2.0;
            for &subingredient in ingredient.2.1.iter() {
                if used.contains(&subingredient.0) {
                    continue;
                }
                used.insert(subingredient.0);
                sorted.push(subingredient);
            }
        }
        sorted.push((item_id, count, recipe));
        (sum, sorted)
    }

    // TODO: use an iterator? - complains about lifetimes though, despite copy data output w/o references
    // TODO: unwrap??
    pub fn sorted<'a>(&self, item_id: u32, recipes_map: &'a HashMap<u32, Recipe>) -> Vec<(u32, u32, &'a Recipe)> {
        self.sort_ingredients(item_id, *self.crafted.get(&item_id).unwrap(), recipes_map).1
    }
}

// Calculate the lowest cost method to obtain the given item, with simulated purchases from
// the trading post.
pub fn calculate_precise_min_crafting_cost(
    item_id: u32,
    item_count: u32,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, Item>,
    tp_listings_map: &mut BTreeMap<u32, profit::ItemListings>,
    context: &mut PreciseCraftingCostContext,
    opt: &config::CraftingOptions,
) -> Option<PreciseCraftingCost> {
    let item = items_map.get(&item_id);
    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let purchases_ptr = context.purchases.len();
    let crafted_backup = context.items.crafted.clone();

    // Take from leftovers first if any
    let (item_count, cost_of_leftovers_used) = if let Some((count, cost, source)) = context.items.leftovers.remove(&item_id) {
        match count.cmp(&item_count) {
            std::cmp::Ordering::Less => {
                // Source is only checked against crafting to break out of profit loop; so prefer
                // whichever other source
                (item_count - count, cost * count)
            },
            std::cmp::Ordering::Equal => {
                return Some(PreciseCraftingCost {
                    cost: cost * item_count,
                    source,
                })
            }
            std::cmp::Ordering::Greater => {
                context.items.leftovers.insert(item_id, (count - item_count, cost, source));
                return Some(PreciseCraftingCost {
                    cost: cost * item_count,
                    source,
                })
            }
        }
    } else {
        (item_count, Money::zero())
    };

    // Craft x, but stash the rest; price is the fraction though
    let crafting_count = Ratio::new(item_count, output_item_count).ceil().to_integer();
    let output_count = crafting_count * output_item_count;

    let leftovers_backup = context.items.leftovers.clone();
    let crafting_cost_per_item = recipe.and_then(|recipe| {
        if !opt.include_timegated && recipe.is_timegated() {
            return None
        }

        let mut cost = Money::zero();
        for ingredient in &recipe.ingredients {
            // adjust ingredient count based on fraction of parent recipe that was requested
            let ingredient_count = ingredient.count * crafting_count;
            let ingredient_cost = calculate_precise_min_crafting_cost(
                ingredient.item_id,
                ingredient_count,
                recipes_map,
                items_map,
                tp_listings_map,
                context,
                opt,
            );
            if let Some(PreciseCraftingCost {
                cost: ingredient_cost,
                ..
            }) = ingredient_cost
            {
                cost += ingredient_cost;
            } else {
                return None;
            }
        }

        // scale to per item
        Some(cost / output_count)
    });
    let crafting_cost = if let Some(cost) = crafting_cost_per_item {
        Some(cost * item_count)
    } else {
        None
    };

    let tp_cost = tp_listings_map
        .get(&item_id)
        .and_then(|listings| listings.lowest_sell_offer(item_count))
        .and_then(|offer| Some(Money::from_copper(offer as i32)));

    let vendor_data = item.and_then(|item| {
        item.vendor_cost()
            .or_else(|| item.token_value().map(|v| (v, 1)))
            .map(|cost| (cost.0 * item_count, cost.1))
    });
    let vendor_cost = if let Some((cost, _)) = vendor_data {
        Some(cost)
    } else {
        None
    };
    let cost = tp_cost.inner_min(crafting_cost).inner_min(vendor_cost)?;

    // give trading post precedence over crafting if costs are equal
    let source = if tp_cost == Some(cost) {
        Source::TradingPost
    } else if crafting_cost == Some(cost) {
        Source::Crafting
    } else {
        Source::Vendor
    };

    if source == Source::Crafting {
        *context.items.crafted.entry(item_id).or_insert(0) += output_count;
        if output_count > item_count {
            // Should never have leftovers if we're crafting more
            debug_assert!(context.items.leftovers.get(&item_id) == None);
            context.items.leftovers.insert(item_id, (
                output_count - item_count, crafting_cost_per_item.unwrap(), Source::Crafting
            ));
        }
    } else {
        // Un-mark ingredients for purchase
        for (purchase_id, purchase_quantity, purchase_source) in
            context.purchases.drain(purchases_ptr..)
        {
            if purchase_source == Source::TradingPost {
                tp_listings_map
                    .get_mut(&purchase_id)
                    .unwrap()
                    .pending_buy_quantity -= purchase_quantity;
            }
        }
        context.items.crafted = crafted_backup;
        context.items.leftovers = leftovers_backup;
    }

    // Mark for purchase
    if source == Source::TradingPost {
        context.purchases.push((item_id, item_count, source));
        tp_listings_map
            .get_mut(&item_id)
            .unwrap()
            .pending_buy_quantity += item_count;
    }
    if source == Source::Vendor {
        let (cost_per_item, purchase_count) = vendor_data.unwrap();
        let purchase = item_count.div_ceil(purchase_count) * purchase_count;
        context.purchases.push((item_id, purchase, source));
        if purchase > item_count {
            // Should never still have leftovers if we're buying more
            debug_assert!(context.items.leftovers.get(&item_id) == None);
            context.items.leftovers.insert(item_id, (purchase - item_count, cost_per_item, Source::Vendor));
        }
    }

    Some(PreciseCraftingCost { cost: cost + cost_of_leftovers_used, source })
}

#[derive(Debug, Eq, PartialEq)]
pub struct PurchasedIngredient {
    pub count: u32,
    pub max_price: Money,
    pub min_price: Money,
    pub total_cost: Money,
}


trait OptionInnerMin<T> {
    fn inner_min(self, other: Option<T>) -> Option<T>;
}

impl<T> OptionInnerMin<T> for Option<T>
where
    T: Ord + Copy,
{
    fn inner_min(self, other: Option<T>) -> Option<T> {
        match (self, other) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (None, _) => other,
            (_, None) => self,
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

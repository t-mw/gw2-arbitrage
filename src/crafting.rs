use crate::api;

use num_rational::Rational32;
use num_traits::Zero;
use rustc_hash::FxHashMap;

pub struct CraftingOptions {
    pub include_timegated: bool,
    pub include_ascended: bool,
    pub count: Option<i32>,
}

// Calculate the lowest cost method to obtain the given item, using only the current high/low tp prices.
// This may involve a combination of crafting, trading and buying from vendors.
pub fn calculate_estimated_min_crafting_cost(
    item_id: u32,
    recipes_map: &FxHashMap<u32, api::Recipe>,
    items_map: &FxHashMap<u32, api::Item>,
    tp_prices_map: &FxHashMap<u32, api::Price>,
    opt: &CraftingOptions,
) -> Option<CraftingCost> {
    let item = items_map.get(&item_id);
    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let crafting_cost = if let Some(recipe) = recipe {
        if !opt.include_timegated && recipe.is_timegated() {
            None
        } else {
            let mut cost = 0;
            for ingredient in &recipe.ingredients {
                let ingredient_cost = calculate_estimated_min_crafting_cost(
                    ingredient.item_id,
                    recipes_map,
                    items_map,
                    tp_prices_map,
                    opt,
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

    let vendor_cost = item.and_then(|item| {
        if opt.include_ascended && item.is_common_ascended_material() {
            Some(0)
        } else {
            item.vendor_cost()
        }
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

    Some(CraftingCost { cost, source })
}

// Calculate the lowest cost method to obtain the given item, with simulated purchases from
// the trading post.
fn calculate_precise_min_crafting_cost(
    item_id: u32,
    recipes_map: &FxHashMap<u32, api::Recipe>,
    items_map: &FxHashMap<u32, api::Item>,
    tp_listings_map: &FxHashMap<u32, api::ItemListings>,
    tp_purchases: &mut Vec<(u32, Rational32)>,
    crafting_steps: &mut Rational32,
    opt: &CraftingOptions,
) -> Option<CraftingCost> {
    let item = items_map.get(&item_id);

    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let tp_purchases_ptr = tp_purchases.len();
    let crafting_steps_before = *crafting_steps;

    let crafting_cost = if let Some(recipe) = recipe {
        if !opt.include_timegated && recipe.is_timegated() {
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
                    opt,
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
                            for (_, count) in
                                tp_purchases.iter_mut().skip(tp_purchases_ingredient_ptr)
                            {
                                *count *= ingredient.count / output_item_count;
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

    let vendor_cost = item.and_then(|item| {
        if opt.include_ascended && item.is_common_ascended_material() {
            Some(0)
        } else {
            item.vendor_cost()
        }
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

#[derive(Debug, Copy, Clone)]
pub struct CraftingCost {
    pub cost: i32,
    source: Source,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum Source {
    Crafting,
    TradingPost,
    Vendor,
}

impl api::ItemListings {
    pub fn calculate_crafting_profit(
        &mut self,
        recipes_map: &FxHashMap<u32, api::Recipe>,
        items_map: &FxHashMap<u32, api::Item>,
        mut tp_listings_map: FxHashMap<u32, api::ItemListings>,
        mut purchased_ingredients: Option<&mut FxHashMap<u32, Rational32>>,
        opt: &CraftingOptions,
    ) -> ProfitableItem {
        let mut listing_profit = 0;
        let mut total_crafting_cost = 0;
        let mut crafting_count = 0;
        let mut total_crafting_steps = Rational32::zero();

        let mut tp_purchases = Vec::with_capacity(512);
        loop {
            if let Some(count) = opt.count {
                if crafting_count >= count {
                    break;
                }
            }

            tp_purchases.clear();
            let mut crafting_steps = Rational32::zero();

            let crafting_cost = if let Some(crafting_cost) = calculate_precise_min_crafting_cost(
                self.id,
                recipes_map,
                items_map,
                &tp_listings_map,
                &mut tp_purchases,
                &mut crafting_steps,
                opt,
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
                        .or_insert_with(Rational32::zero);
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

#[derive(Debug)]
pub struct ProfitableItem {
    pub id: u32,
    crafting_cost: i32,
    pub crafting_steps: Rational32,
    pub count: i32,
    pub profit: i32,
}

impl ProfitableItem {
    pub fn profit_per_item(&self) -> i32 {
        self.profit / self.count
    }

    pub fn profit_per_crafting_step(&self) -> i32 {
        (Rational32::from_integer(self.profit) / self.crafting_steps)
            .floor()
            .to_integer()
    }

    pub fn profit_on_cost(&self) -> Rational32 {
        Rational32::new(self.profit, self.crafting_cost)
    }
}

// integer division rounding up
// see: https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
fn div_i32_ceil(x: i32, y: i32) -> i32 {
    (x + y - 1) / y
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

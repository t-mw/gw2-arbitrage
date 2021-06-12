use crate::api;

use num_rational::Rational32;
use num_traits::{Signed, Zero};

use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Default)]
pub struct CraftingOptions {
    pub include_timegated: bool,
    pub include_ascended: bool,
    pub count: Option<i32>,
}

#[derive(Debug, Copy, Clone)]
pub struct EstimatedCraftingCost {
    pub cost: i32,
    source: Source,
}

// Calculate the lowest cost method to obtain the given item, using only the current high/low tp prices.
// This may involve a combination of crafting, trading and buying from vendors.
pub fn calculate_estimated_min_crafting_cost(
    item_id: u32,
    recipes_map: &HashMap<u32, api::Recipe>,
    items_map: &HashMap<u32, api::Item>,
    tp_prices_map: &HashMap<u32, api::Price>,
    opt: &CraftingOptions,
) -> Option<EstimatedCraftingCost> {
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

    Some(EstimatedCraftingCost { cost, source })
}

#[derive(Debug, Copy, Clone)]
struct PreciseCraftingCost {
    cost: Rational32,
    source: Source,
}

// Calculate the lowest cost method to obtain the given item, with simulated purchases from
// the trading post.
fn calculate_precise_min_crafting_cost(
    item_id: u32,
    item_count: Rational32,
    recipes_map: &HashMap<u32, api::Recipe>,
    items_map: &HashMap<u32, api::Item>,
    tp_listings_map: &mut BTreeMap<u32, ItemListings>,
    purchases: &mut Vec<(u32, Rational32, Source)>,
    crafting_steps: &mut Rational32,
    opt: &CraftingOptions,
) -> Option<PreciseCraftingCost> {
    let item = items_map.get(&item_id);
    let recipe = recipes_map.get(&item_id);
    let output_item_count =
        Rational32::from(recipe.map(|recipe| recipe.output_item_count).unwrap_or(1));

    let purchases_ptr = purchases.len();
    let crafting_steps_before = *crafting_steps;

    let crafting_cost = recipe.and_then(|recipe| {
        if !opt.include_timegated && recipe.is_timegated() {
            None
        } else {
            let mut cost = Rational32::zero();
            for ingredient in &recipe.ingredients {
                // adjust ingredient count based on fraction of parent recipe that was requested
                let ingredient_count =
                    Rational32::from(ingredient.count) * (item_count / output_item_count);
                let ingredient_cost = calculate_precise_min_crafting_cost(
                    ingredient.item_id,
                    ingredient_count,
                    recipes_map,
                    items_map,
                    tp_listings_map,
                    purchases,
                    crafting_steps,
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
            Some(cost)
        }
    });

    let tp_cost = tp_listings_map
        .get(&item_id)
        .and_then(|listings| listings.lowest_sell_offer(item_count));
    let vendor_cost = item.and_then(|item| {
        if opt.include_ascended && item.is_common_ascended_material() {
            Some(Rational32::zero())
        } else {
            item.vendor_cost()
                .map(|cost| Rational32::from(cost) * item_count)
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

    if source == Source::Crafting {
        *crafting_steps += item_count / output_item_count;
    } else {
        for (purchase_id, purchase_quantity, purchase_source) in purchases.drain(purchases_ptr..) {
            if purchase_source == Source::TradingPost {
                tp_listings_map
                    .get_mut(&purchase_id)
                    .unwrap()
                    .pending_buy_quantity -= purchase_quantity;
            }
        }
        *crafting_steps = crafting_steps_before;
    }

    if source != Source::Crafting {
        purchases.push((item_id, item_count, source));
    }
    if source == Source::TradingPost {
        tp_listings_map
            .get_mut(&item_id)
            .unwrap()
            .pending_buy_quantity += item_count;
    }

    Some(PreciseCraftingCost { cost, source })
}

#[derive(Debug, Copy, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Source {
    Crafting,
    TradingPost,
    Vendor,
}

pub fn calculate_crafting_profit(
    item_id: u32,
    recipes_map: &HashMap<u32, api::Recipe>,
    items_map: &HashMap<u32, api::Item>,
    tp_listings_map: &HashMap<u32, api::ItemListings>,
    mut purchased_ingredients: Option<&mut HashMap<(u32, Source), Rational32>>,
    opt: &CraftingOptions,
) -> Option<ProfitableItem> {
    let mut tp_listings_map: BTreeMap<u32, ItemListings> = tp_listings_map
        .clone()
        .into_iter()
        .map(|(id, listings)| (id, ItemListings::from(listings)))
        .collect();

    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let mut listing_profit = Rational32::zero();
    let mut total_crafting_cost = Rational32::zero();
    let mut crafting_count = 0;
    let mut total_crafting_steps = Rational32::zero();

    // simulate crafting 1 item per loop iteration until it becomes unprofitable
    let mut purchases = Vec::new();
    loop {
        if let Some(count) = opt.count {
            if crafting_count + output_item_count > count {
                break;
            }
        }

        purchases.clear();
        let mut crafting_steps = Rational32::zero();

        let crafting_cost = if let Some(PreciseCraftingCost {
            source: Source::Crafting,
            cost,
        }) = calculate_precise_min_crafting_cost(
            item_id,
            output_item_count.into(),
            recipes_map,
            items_map,
            &mut tp_listings_map,
            &mut purchases,
            &mut crafting_steps,
            opt,
        ) {
            cost
        } else {
            break;
        };

        let buy_price = if let Some(buy_price) = tp_listings_map
            .get_mut(&item_id)
            .unwrap_or_else(|| panic!("Missing listings for item id: {}", item_id))
            .sell(output_item_count.into())
        {
            buy_price
        } else {
            break;
        };

        let profit = buy_price - crafting_cost;
        if profit.is_positive() {
            listing_profit += profit;
            total_crafting_cost += crafting_cost;
            crafting_count += output_item_count;
        } else {
            break;
        }

        for (purchase_id, count, purchase_source) in &purchases {
            if *purchase_source != Source::TradingPost {
                continue;
            }

            let listing = tp_listings_map.get_mut(purchase_id).unwrap_or_else(|| {
                panic!(
                    "Missing listings for ingredient {} of item id {}",
                    purchase_id, item_id
                )
            });
            listing.pending_buy_quantity -= *count;
            listing.buy(*count).unwrap_or_else(|| {
                panic!(
                    "Expected to be able to buy {} of ingredient {} for item id {}",
                    count, purchase_id, item_id
                )
            });
        }
        debug_assert!(tp_listings_map
            .iter()
            .all(|(_, listing)| listing.pending_buy_quantity.is_zero()));

        if let Some(purchased_ingredients) = &mut purchased_ingredients {
            for (purchase_id, count, purchase_source) in &purchases {
                let existing_count = purchased_ingredients
                    .entry((*purchase_id, *purchase_source))
                    .or_insert_with(Rational32::zero);
                *existing_count += count;
            }
        }

        total_crafting_steps += crafting_steps;
    }

    if crafting_count > 0 && listing_profit.is_positive() {
        Some(ProfitableItem {
            id: item_id,
            crafting_cost: total_crafting_cost,
            crafting_steps: total_crafting_steps,
            profit: listing_profit,
            count: crafting_count,
        })
    } else {
        None
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProfitableItem {
    pub id: u32,
    pub crafting_cost: Rational32,
    pub crafting_steps: Rational32,
    pub count: i32,
    pub profit: Rational32,
}

impl ProfitableItem {
    pub fn profit_per_item(&self) -> Rational32 {
        self.profit / Rational32::from(self.count)
    }

    pub fn profit_per_crafting_step(&self) -> Rational32 {
        self.profit / self.crafting_steps
    }

    pub fn profit_on_cost(&self) -> Rational32 {
        self.profit / self.crafting_cost
    }
}

#[derive(Clone, Debug)]
pub struct ItemListings {
    pub id: u32,
    pub buys: Vec<Listing>,
    pub sells: Vec<Listing>,
    pub pending_buy_quantity: Rational32,
}

#[derive(Clone, Debug)]
pub struct Listing {
    pub unit_price: i32,
    pub quantity: Rational32,
}

impl ItemListings {
    fn buy(&mut self, mut count: Rational32) -> Option<Rational32> {
        let mut cost = Rational32::zero();

        while count.is_positive() {
            // sells are sorted in descending price
            let remove = if let Some(listing) = self.sells.last_mut() {
                listing.quantity -= Rational32::from(1);
                count -= Rational32::from(1);
                cost += listing.unit_price;
                listing.quantity.is_zero()
            } else {
                return None;
            };

            if remove {
                self.sells.pop();
            }
        }

        Some(cost)
    }

    fn sell(&mut self, mut count: Rational32) -> Option<Rational32> {
        let mut revenue = Rational32::zero();

        while count.is_positive() {
            // buys are sorted in ascending price
            let remove = if let Some(listing) = self.buys.last_mut() {
                listing.quantity -= Rational32::from(1);
                count -= Rational32::from(1);
                revenue += listing.unit_price_minus_fees();
                listing.quantity.is_zero()
            } else {
                return None;
            };

            if remove {
                self.buys.pop();
            }
        }

        Some(revenue)
    }

    fn lowest_sell_offer(&self, mut quantity: Rational32) -> Option<Rational32> {
        debug_assert!(!quantity.is_zero());

        let mut cost = Rational32::zero();
        let mut pending_buy_quantity = self.pending_buy_quantity;

        for listing in self.sells.iter().rev() {
            let mut remaining_listing_quantity = listing.quantity;
            if pending_buy_quantity.is_positive() {
                pending_buy_quantity -= remaining_listing_quantity;
                remaining_listing_quantity = -pending_buy_quantity;
            }

            if remaining_listing_quantity.is_positive() {
                if remaining_listing_quantity < quantity {
                    quantity -= remaining_listing_quantity;
                    cost += remaining_listing_quantity * listing.unit_price;
                } else {
                    cost += quantity * listing.unit_price;
                    quantity = Rational32::zero();
                }
            }

            if quantity.is_zero() {
                break;
            }
        }

        if quantity.is_positive() {
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
            pending_buy_quantity: Rational32::zero(),
        }
    }
}

impl Listing {
    pub fn unit_price_minus_fees(&self) -> Rational32 {
        api::apply_trading_post_sales_commission(self.unit_price)
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

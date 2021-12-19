use crate::api;
use crate::gw2efficiency;
use crate::config;
use crate::money;

use serde::{Deserialize, Serialize};

use num_rational::Ratio;
use num_traits::Zero;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::TryFrom;

#[derive(Debug, Copy, Clone)]
pub struct EstimatedCraftingCost {
    pub cost: money::Money,
    pub source: Source,
}

// Calculate the lowest cost method to obtain the given item, using only the current high/low tp prices.
// This may involve a combination of crafting, trading and buying from vendors.
pub fn calculate_estimated_min_crafting_cost(
    item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, api::Item>,
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
            let mut cost = money::Money::zero();
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

            Some(cost.div_u32_ceil(output_item_count))
        }
    });

    let tp_cost = tp_prices_map
        .get(&item_id)
        .filter(|price| price.sells.quantity > 0)
        .map(|price| money::Money::from_copper(price.sells.unit_price));

    let vendor_cost = item.and_then(|item| {
        if opt.include_ascended && item.is_common_ascended_material() {
            Some(money::Money::zero())
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
    cost: money::Money,
    source: Source,
}

struct PreciseCraftingCostContext {
    purchases: Vec<(u32, u32, Source)>,
    crafted: Vec<u32>,
    crafting_steps: u32,
    leftovers: HashMap<u32, (u32, money::Money)>,
}

// Calculate the lowest cost method to obtain the given item, with simulated purchases from
// the trading post.
fn calculate_precise_min_crafting_cost(
    item_id: u32,
    item_count: u32,
    recipes_map: &HashMap<u32, Recipe>,
    items_map: &HashMap<u32, api::Item>,
    tp_listings_map: &mut BTreeMap<u32, ItemListings>,
    context: &mut PreciseCraftingCostContext,
    opt: &config::CraftingOptions,
) -> Option<PreciseCraftingCost> {
    let item = items_map.get(&item_id);
    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);

    let purchases_ptr = context.purchases.len();
    let crafted_ptr = context.crafted.len();
    let crafting_steps_before = context.crafting_steps;

    // Take from leftovers first if any
    let (item_count, cost_of_leftovers_used) = if let Some((count, cost)) = context.leftovers.remove(&item_id) {
        match count.cmp(&item_count) {
            std::cmp::Ordering::Less => {
                // Source is only checked against crafting to break out of profit loop; so prefer
                // whichever other source
                context.leftovers.remove(&item_id);
                (item_count - count, cost * count)
            },
            std::cmp::Ordering::Equal => {
                context.leftovers.remove(&item_id);
                return Some(PreciseCraftingCost {
                    cost: cost * item_count,
                    source: Source::Crafting,
                })
            }
            std::cmp::Ordering::Greater => {
                context.leftovers.insert(item_id, (count - item_count, cost));
                return Some(PreciseCraftingCost {
                    cost: cost * item_count,
                    source: Source::Crafting,
                })
            }
        }
    } else {
        (item_count, money::Money::zero())
    };

    // Craft x, but stash the rest; price is the fraction though
    let crafting_cost = recipe.and_then(|recipe| {
        if !opt.include_timegated && recipe.is_timegated() {
            return None
        }

        let mut cost = money::Money::zero();
        for ingredient in &recipe.ingredients {
            // adjust ingredient count based on fraction of parent recipe that was requested
            let ingredient_count = Ratio::new(ingredient.count * item_count, output_item_count).ceil().to_integer();
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

        Some(cost)
    });

    let tp_cost = tp_listings_map
        .get(&item_id)
        .and_then(|listings| listings.lowest_sell_offer(item_count))
        .and_then(|offer| Some(money::Money::from_copper(offer)));

    let vendor_cost = item.and_then(|item| {
        if opt.include_ascended && item.is_common_ascended_material() {
            Some(money::Money::zero())
        } else {
            item.vendor_cost()
                .map(|cost| cost * item_count)
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
        context.crafted.push(item_id);
        context.crafting_steps += Ratio::new(item_count, output_item_count).ceil().to_integer();
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
        context.crafted.drain(crafted_ptr..);
        context.crafting_steps = crafting_steps_before;
    }

    // Mark for purchase
    if source != Source::Crafting {
        context.purchases.push((item_id, item_count, source));
    }
    if source == Source::TradingPost {
        tp_listings_map
            .get_mut(&item_id)
            .unwrap()
            .pending_buy_quantity += item_count;
    }

    Some(PreciseCraftingCost { cost: cost + cost_of_leftovers_used, source })
}

#[derive(Debug, Copy, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Source {
    Crafting,
    TradingPost,
    Vendor,
}

pub fn calculate_crafting_profit(
    item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    known_recipes: &Option<HashSet<u32>>,
    items_map: &HashMap<u32, api::Item>,
    tp_listings_map: &HashMap<u32, api::ItemListings>,
    mut purchased_ingredients: Option<&mut HashMap<(u32, Source), PurchasedIngredient>>,
    opt: &config::CraftingOptions,
) -> Option<ProfitableItem> {
    let mut tp_listings_map: BTreeMap<u32, ItemListings> = tp_listings_map
        .clone()
        .into_iter()
        .map(|(id, listings)| (id, ItemListings::from(listings)))
        .collect();

    let recipe = recipes_map.get(&item_id);
    let output_item_count = recipe.map(|recipe| recipe.output_item_count).unwrap_or(1);
    let threshold = money::Money::from_copper(opt.threshold.unwrap_or(0));

    let mut listing_profit = money::Money::zero();
    let mut total_crafting_cost = money::Money::zero();
    let mut crafting_count = 0;
    let mut total_crafting_steps = 0;
    let mut unknown_recipes = HashSet::new();

    let mut min_sell = 0;
    let max_sell = tp_listings_map
        .get(&item_id)
        .unwrap_or_else(|| panic!("Missing listings for item id: {}", item_id))
        .buys
        .last()
        .map_or(0, |l| l.unit_price);
    let mut breakeven = money::Money::zero();

    // simulate crafting 1 item per loop iteration until it becomes unprofitable
    loop {
        if let Some(count) = opt.count {
            if crafting_count + output_item_count > count {
                break;
            }
        }

        let mut context = PreciseCraftingCostContext {
            purchases: vec![],
            crafted: vec![],
            crafting_steps: 0,
            leftovers: HashMap::new(),
        };

        let crafting_cost = if let Some(PreciseCraftingCost {
            source: Source::Crafting,
            cost,
        }) = calculate_precise_min_crafting_cost(
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
            (money::Money::from_copper(price), price)
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
        if buy_price >= crafting_cost + threshold {
            listing_profit += buy_price - crafting_cost;
            total_crafting_cost += crafting_cost;
            crafting_count += output_item_count;

            min_sell = min_buy;
            // Breakeven is based on the last/most expensive to craft
            breakeven = crafting_cost / output_item_count;
        } else {
            break;
        }

        for item_id in &context.crafted {
            let recipe = if let Some(recipe) = recipes_map.get(item_id) {
                recipe
            } else {
                continue;
            };
            if let Some(id) = recipe.id.filter(|id| !unknown_recipes.contains(id)) {
                match recipe.source {
                    RecipeSource::Purchasable | RecipeSource::Achievement => {
                        // If we have no known recipes, assume we know none
                        if known_recipes
                            .as_ref()
                            .filter(|recipes| recipes.contains(&id))
                            .is_none()
                        {
                            unknown_recipes.insert(id);
                        }
                    }
                    // These aren't included in the API; assume you know them
                    RecipeSource::Automatic | RecipeSource::Discoverable => {
                        // TODO: instead, check if account has a char with the required crafting level
                        // Would require a key with the characters scope. Still wouldn't detect
                        // discoverable recipes, but would detect access to them
                    }
                }
            }
        }

        for (purchase_id, count, purchase_source) in &context.purchases {
            let (cost, min_sell, max_sell) = if let Source::TradingPost = *purchase_source {
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
                    .or_insert_with(|| PurchasedIngredient {
                        count: 0,
                        max_price: money::Money::default(),
                        min_price: money::Money::default(),
                        total_cost: money::Money::default(),
                    });
                ingredient.count += count;
                if ingredient.min_price.is_zero() {
                    ingredient.min_price = money::Money::from_copper(min_sell);
                }
                ingredient.max_price = money::Money::from_copper(max_sell);
                ingredient.total_cost += money::Money::from_copper(cost);
            }
        }
        debug_assert!(tp_listings_map
            .iter()
            .all(|(_, listing)| listing.pending_buy_quantity == 0));

        total_crafting_steps += context.crafting_steps;
    }

    if crafting_count > 0 && !listing_profit.is_zero() {
        Some(ProfitableItem {
            id: item_id,
            crafting_cost: total_crafting_cost,
            crafting_steps: total_crafting_steps,
            profit: listing_profit,
            count: crafting_count,
            unknown_recipes,
            max_sell: money::Money::from_copper(max_sell),
            min_sell: money::Money::from_copper(min_sell),
            breakeven: breakeven.trading_post_listing_price(),
        })
    } else {
        None
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PurchasedIngredient {
    pub count: u32,
    pub max_price: money::Money,
    pub min_price: money::Money,
    pub total_cost: money::Money,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProfitableItem {
    pub id: u32,
    pub crafting_cost: money::Money,
    pub crafting_steps: u32,
    pub count: u32,
    pub profit: money::Money,
    pub unknown_recipes: HashSet<u32>, // id
    pub max_sell: money::Money,
    pub min_sell: money::Money,
    pub breakeven: money::Money,
}

impl ProfitableItem {
    pub fn profit_per_item(&self) -> money::Money {
        self.profit / self.count
    }

    pub fn profit_per_crafting_step(&self) -> money::Money {
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

    fn sell(&mut self, mut count: u32) -> Option<(money::Money, u32)> {
        let mut revenue = money::Money::zero();
        let mut min_buy = 0;

        while count > 0 {
            // buys are sorted in ascending price
            let remove = if let Some(listing) = self.buys.last_mut() {
                listing.quantity -= 1;
                count -= 1;
                min_buy = listing.unit_price;
                revenue += money::Money::from_copper(listing.unit_price).trading_post_sale_revenue();
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

    fn lowest_sell_offer(&self, mut quantity: u32) -> Option<u32> {
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

#[derive(Debug, Serialize, Deserialize)]
pub enum RecipeSource {
    Automatic,
    Discoverable,
    Purchasable,
    Achievement,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub id: Option<u32>,
    pub output_item_id: u32,
    pub output_item_count: u32,
    pub disciplines: Vec<config::Discipline>,
    pub ingredients: Vec<api::RecipeIngredient>,
    source: RecipeSource,
}

impl From<api::Recipe> for Recipe {
    fn from(recipe: api::Recipe) -> Self {
        let source = if recipe.is_purchased() {
            RecipeSource::Purchasable
        } else if recipe.is_automatic() {
            RecipeSource::Automatic
        } else {
            RecipeSource::Discoverable
        };
        Recipe {
            id: Some(recipe.id),
            output_item_id: recipe.output_item_id,
            output_item_count: recipe.output_item_count,
            disciplines: recipe.disciplines,
            ingredients: recipe.ingredients,
            source,
        }
    }
}

impl TryFrom<gw2efficiency::Recipe> for Recipe {
    type Error = String;

    fn try_from(recipe: gw2efficiency::Recipe) -> Result<Self, Self::Error> {
        let output_item_count = if let Some(count) = recipe.output_item_count {
            count
        } else {
            return Err(format!(
                "Ignoring '{}'. Failed to parse 'output_item_count' as integer.",
                recipe.name
            ));
        };
        // Any disciplines _except_ Achievement can be counted as known
        // While some regular discipline precursor recipes must be learned, the
        // outputs appear to be account bound anyway, so won't be on TP.
        // There are some useful Scribe WvW BPs in the data, so ignoring all
        // normal discipline recipes would catch those too.
        let source = if recipe
            .disciplines
            .contains(&config::Discipline::Achievement)
        {
            RecipeSource::Achievement
        } else {
            RecipeSource::Automatic
        };
        Ok(Recipe {
            id: None,
            output_item_id: recipe.output_item_id,
            output_item_count,
            disciplines: recipe.disciplines,
            ingredients: recipe.ingredients,
            source,
        })
    }
}

impl Recipe {
    // see https://wiki.guildwars2.com/wiki/Category:Time_gated_recipes
    // for a list of time gated recipes
    // I've left Charged Quartz Crystals off the list, since they can
    // drop from containers.
    pub fn is_timegated(&self) -> bool {
        self.output_item_id == 46740         // Spool of Silk Weaving Thread
            || self.output_item_id == 46742  // Lump of Mithrillium
            || self.output_item_id == 46744  // Glob of Elder Spirit Residue
            || self.output_item_id == 46745  // Spool of Thick Elonian Cord
            || self.output_item_id == 66913  // Clay Pot
            || self.output_item_id == 66917  // Plate of Meaty Plant Food
            || self.output_item_id == 66923  // Plate of Piquant Plan Food
            || self.output_item_id == 67015  // Heat Stone
            || self.output_item_id == 67377  // Vial of Maize Balm
            || self.output_item_id == 79726  // Dragon Hatchling Doll Eye
            || self.output_item_id == 79763  // Gossamer Stuffing
            || self.output_item_id == 79790  // Dragon Hatchling Doll Hide
            || self.output_item_id == 79795  // Dragon Hatchling Doll Adornments
            || self.output_item_id == 79817  // Dragon Hatchling Doll Frame
            || self.output_item_id == 43772 // Charged Quartz Crystal
    }

    #[cfg(test)]
    pub(crate) fn mock<const A: usize>(
        id: u32,
        output_item_id: u32,
        output_item_count: u32,
        disciplines: [config::Discipline; A],
        ingredients: &[api::RecipeIngredient],
        source: RecipeSource,
    ) -> Self {
        Recipe {
            id: Some(id),
            output_item_id,
            output_item_count,
            disciplines: disciplines.to_vec(),
            ingredients: ingredients.to_vec(),
            source,
        }
    }
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

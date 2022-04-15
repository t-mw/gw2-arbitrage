use crate::api;
use crate::gw2efficiency;
use crate::config;
use crate::money::Money;

use serde::{Deserialize, Serialize};

use num_rational::Ratio;
use num_traits::Zero;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::TryFrom;
use std::cmp::Ordering;

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
struct PreciseCraftingCost {
    cost: Money,
    source: Source,
}

struct PreciseCraftingCostContext {
    purchases: Vec<(u32, u32, Source)>, // id, count, Source
    items: CraftedItems,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct CraftedItems {
    pub crafted: HashMap<u32, u32>, // id, count
    pub leftovers: HashMap<u32, (u32, Money, Source)>,
}

impl CraftedItems {
    fn crafting_steps(
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

pub fn calculate_crafting_profit(
    item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
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
    let threshold = Money::from_copper(opt.threshold.unwrap_or(0) as i32);

    let mut listing_profit = Money::zero();
    let mut total_crafting_cost = Money::zero();
    let mut crafting_count = 0;
    let mut crafted_items = CraftedItems::default();

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

        let mut context = PreciseCraftingCostContext {
            purchases: vec![],
            items: crafted_items.clone(),
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
            (Money::from_copper(price as i32), price)
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
pub struct PurchasedIngredient {
    pub count: u32,
    pub max_price: Money,
    pub min_price: Money,
    pub total_cost: Money,
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
    pub crafted_items: CraftedItems,
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

    pub fn sorted_ingredients(&self) -> Vec<&api::RecipeIngredient> {
        let mut ingredients: Vec<&api::RecipeIngredient> = self.ingredients.iter().collect();
        ingredients.sort_unstable_by(|a, b| {
            match b.count.cmp(&a.count) {
                Ordering::Equal => b.item_id.cmp(&a.item_id),
                v => v,
            }
        });
        ingredients
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

trait DivCeil {
    fn div_ceil(&self, other: Self) -> Self;
}
impl DivCeil for u32 {
    fn div_ceil(&self, other: Self) -> Self {
        (self + other - 1) / other
    }
}

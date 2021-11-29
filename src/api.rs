use num_rational::Rational32;
use serde::{Deserialize, Serialize};
use std::fmt;

use phf::{phf_set, phf_map};

const TRADING_POST_SALES_COMMISSION: i32 = 15; // %

pub fn subtract_trading_post_sales_commission(v: i32) -> Rational32 {
    Rational32::new(100 - TRADING_POST_SALES_COMMISSION, 100) * v
}
pub fn add_trading_post_sales_commission(v: Rational32) -> i32 {
    (v / Rational32::new(100 - TRADING_POST_SALES_COMMISSION, 100)).to_integer()
}

// types for /commerce/prices
#[derive(Debug, Serialize, Deserialize)]
pub struct Price {
    pub id: u32,
    pub buys: PriceInfo,
    pub sells: PriceInfo,
}

impl Price {
    pub fn effective_buy_price(&self) -> i32 {
        (self.buys.unit_price as f32 * (1.0 - TRADING_POST_SALES_COMMISSION as f32 / 100.0)).floor()
            as i32
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PriceInfo {
    pub unit_price: i32,
    pub quantity: i32,
}

// types for /recipes
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Recipe {
    pub id: u32,
    #[serde(rename = "type")]
    type_name: String,
    pub output_item_id: u32,
    pub output_item_count: i32,
    time_to_craft_ms: i32,
    pub disciplines: Vec<String>,
    min_rating: i32,
    flags: Vec<String>,
    pub ingredients: Vec<RecipeIngredient>,
    chat_link: String,
}

impl Recipe {
    pub fn is_purchased(&self) -> bool {
        self.flags.contains(&"LearnedFromItem".to_string())
    }
    pub fn is_automatic(&self) -> bool {
        self.flags.contains(&"AutoLearned".to_string())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RecipeIngredient {
    pub item_id: u32,
    pub count: i32,
}

// types for /items
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Item {
    pub id: u32,
    chat_link: String,
    pub name: String,
    #[serde(rename = "type")]
    type_name: String,
    rarity: String,
    level: i32,
    vendor_value: i32,
    flags: Vec<String>,
    restrictions: Vec<String>,
    upgrades_into: Option<Vec<ItemUpgrade>>,
    upgrades_from: Option<Vec<ItemUpgrade>>,
}

// NOTE: most can only be purchased in blocks of 10 - we ignore that for now
// NOTE: doesn't include karma purchases, since the karma to gold rate is undefined and we don't
// support multiple currencies
static VENDOR_ITEMS: phf::Set<u32> = phf_set! {
    19792_u32, // Spool of Jute Thread - 10
    19789_u32, // Spool of Wool Thread - 10
    19794_u32, // Spool of Cotton Thread - 10
    19793_u32, // Spool of Linen Thread - 10
    19791_u32, // Spool of Silk Thread - 10
    19790_u32, // Spool of Gossamer Thread - 10
    13010_u32, // Minor Rune of Holding
    13006_u32, // Rune of Holding
    13007_u32, // Major Rune of Holding
    13008_u32, // Greater Rune of Holding
    13009_u32, // Superior Rune of Holding
    19704_u32, // Lump of Tin - 10
    19750_u32, // Lump of Coal - 10
    19924_u32, // Lump of Primordium - 10
    12157_u32, // Jar of Vinegar - 10
    12151_u32, // Packet of Baking Powder - 10
    12158_u32, // Jar of Vegetable Oil - 10
    12153_u32, // Packet of Salt - 10
    12155_u32, // Bag of Sugar - 10
    12156_u32, // Jug of Water - 10 - only 10?
    12324_u32, // Bag of Starch - 10
    12136_u32, // Bag of Flour - 1, from some vendors, 10 from master chefs
    12271_u32, // Bottle of Soy Sauce - 10
    76839_u32, // Milling Basin - can buy one at a time from chefs and scribe
    70647_u32, // Crystalline Bottle - can buy one at a time from master scribe
    75762_u32, // Bag of Mortar - can buy one at a time from master scribe
    75087_u32, // Essence of Elegance - buy one at a time
};
// Sell price is _not_ buy price * 8
static SPECIAL_VENDOR_ITEMS: phf::Map<u32, i32> = phf_map! {
    46747_u32 => 150, // Thermocatalytic Reagent - 1496 for 10
    91739_u32 => 150, // Pile of Compost Starter - 1496 for 10
    91702_u32 => 200, // Pile of Powdered Gelatin Mix - 5 for 1000; prereq achievement
    90201_u32 => 40000, // Smell-Enhancing Culture; prereq achievement
};
impl Item {
    pub fn vendor_cost(&self) -> Option<i32> {
        if VENDOR_ITEMS.contains(&self.id) {
            if self.vendor_value > 0 {
                // standard vendor sell price is generally buy price * 8, see:
                //  https://forum-en.gw2archive.eu/forum/community/api/How-to-get-the-vendor-sell-price
                Some(self.vendor_value * 8)
            } else {
                None
            }
        } else if SPECIAL_VENDOR_ITEMS.contains_key(&self.id) {
            Some(SPECIAL_VENDOR_ITEMS[&self.id])
        } else {
            None
        }
    }

    pub fn is_restricted(&self) -> bool {
        // 76363 == legacy catapult schematic
        self.id == 76363
            || self
                .flags
                .iter()
                .any(|flag| *flag == "AccountBound" || *flag == "SoulbindOnAcquire")
    }

    pub fn is_common_ascended_material(&self) -> bool {
        // Empyreal Fragment, Dragonite Ore, Pile of Bloodstone Dust
        self.id == 46735 || self.id == 46733 || self.id == 46731
    }

    #[cfg(test)]
    pub(crate) fn mock(id: u32, name: &str, vendor_value: i32) -> Self {
        Item {
            id,
            name: name.to_string(),
            vendor_value,
            ..Default::default()
        }
    }
}

// When printing an item, add rarity if a trinket, as most trinkets use the same
// name for different rarities
impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if &self.type_name == "Trinket" {
            write!(f, "{} ({})", &self.name, &self.rarity)
        } else {
            write!(f, "{}", &self.name)
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ItemUpgrade {
    upgrade: String,
    item_id: i32,
}

// types for /commerce/listings
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemListings {
    pub id: u32,
    pub buys: Vec<Listing>,
    pub sells: Vec<Listing>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Listing {
    pub listings: i32,
    pub unit_price: i32,
    pub quantity: i32,
}

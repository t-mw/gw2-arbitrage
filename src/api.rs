use num_rational::Rational32;
use serde::{Deserialize, Serialize};
use std::fmt;

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

impl Item {
    pub fn vendor_cost(&self) -> Option<i32> {
        let name = &self.name;

        if name == "Thermocatalytic Reagent" {
            Some(150)
        } else if name == "Spool of Jute Thread"
            || name == "Spool of Wool Thread"
            || name == "Spool of Cotton Thread"
            || name == "Spool of Linen Thread"
            || name == "Spool of Silk Thread"
            || name == "Spool of Gossamer Thread"
            || (name.ends_with("Rune of Holding") && !name.starts_with("Supreme"))
            || name == "Lump of Tin"
            || name == "Lump of Coal"
            || name == "Lump of Primordium"
            || name == "Jar of Vinegar"
            || name == "Packet of Baking Powder"
            || name == "Jar of Vegetable Oil"
            || name == "Packet of Salt"
            || name == "Bag of Sugar"
            || name == "Jug of Water"
            || name == "Bag of Starch"
            || name == "Bag of Flour"
            || name == "Bottle of Soy Sauce"
            || name == "Milling Basin"
            || name == "Crystalline Bottle"
            || name == "Bag of Mortar"
            || name == "Essence of Elegance"
        {
            if self.vendor_value > 0 {
                // standard vendor sell price is generally buy price * 8, see:
                //  https://forum-en.gw2archive.eu/forum/community/api/How-to-get-the-vendor-sell-price
                Some(self.vendor_value * 8)
            } else {
                None
            }
        } else if name == "Pile of Compost Starter" {
            Some(150)
        } else if name == "Pile of Powdered Gelatin Mix" {
            Some(200)
        } else if name == "Smell-Enhancing Culture" {
            Some(40000)
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
        let name = &self.name;
        name == "Empyreal Fragment" || name == "Dragonite Ore" || name == "Pile of Bloodstone Dust"
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

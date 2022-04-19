use serde::{Deserialize, Serialize};

use crate::config;
use crate::item;

// types for /commerce/prices
#[derive(Debug, Serialize, Deserialize)]
pub struct Price {
    pub id: u32,
    pub buys: PriceInfo,
    pub sells: PriceInfo,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PriceInfo {
    pub unit_price: u32,
    pub quantity: u32,
}

// types for /recipes
#[derive(Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub id: u32,
    pub output_item_id: u32,
    pub output_item_count: u32,
    time_to_craft_ms: u32,
    pub disciplines: Vec<config::Discipline>,
    min_rating: u16,
    flags: Vec<RecipeFlags>,
    pub ingredients: Vec<RecipeIngredient>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum RecipeFlags {
    AutoLearned,
    LearnedFromItem,
}

impl Recipe {
    pub fn is_purchased(&self) -> bool {
        self.flags.contains(&RecipeFlags::LearnedFromItem)
    }
    pub fn is_automatic(&self) -> bool {
        self.flags.contains(&RecipeFlags::AutoLearned)
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct RecipeIngredient {
    pub item_id: u32,
    pub count: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiItem {
    pub id: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub item_type: item::Type,
    pub rarity: item::Rarity,
    pub level: i32,
    pub vendor_value: u32,
    pub flags: Vec<item::Flag>,
    pub restrictions: Vec<String>,
    pub upgrades_into: Option<Vec<item::Upgrade>>,
    pub upgrades_from: Option<Vec<item::Upgrade>>,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
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
    pub listings: u32,
    pub unit_price: u32,
    pub quantity: u32,
}

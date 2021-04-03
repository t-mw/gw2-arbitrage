use serde::{Deserialize, Serialize};

const TRADING_POST_COMMISSION: f32 = 0.15;

// types for /commerce/prices
#[derive(Debug, Serialize, Deserialize)]
pub struct Price {
    pub id: u32,
    pub buys: PriceInfo,
    pub sells: PriceInfo,
}

impl Price {
    pub fn effective_buy_price(&self) -> i32 {
        (self.buys.unit_price as f32 * (1.0 - TRADING_POST_COMMISSION)).floor() as i32
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PriceInfo {
    pub unit_price: i32,
    pub quantity: i32,
}

// types for /recipes
#[derive(Debug, Serialize, Deserialize)]
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
            || self.output_item_id == 79817 // Dragon Hatchling Doll Frame
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RecipeIngredient {
    pub item_id: u32,
    pub count: i32,
}

// types for /items
#[derive(Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: u32,
    chat_link: String,
    pub name: String,
    icon: Option<String>,
    description: Option<String>,
    #[serde(rename = "type")]
    type_name: String,
    rarity: String,
    level: i32,
    vendor_value: i32,
    default_skin: Option<i32>,
    flags: Vec<String>,
    game_types: Vec<String>,
    restrictions: Vec<String>,
    upgrades_into: Option<Vec<ItemUpgrade>>,
    upgrades_from: Option<Vec<ItemUpgrade>>,
}

impl Item {
    pub fn vendor_cost(&self) -> Option<i32> {
        let name = &self.name;

        if name == "Thermocatalytic Reagent"
            || name == "Spool of Jute Thread"
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
        // 24749 == legacy Major Rune of the Air
        // 76363 == legacy catapult schematic
        self.id == 24749
            || self.id == 76363
            || self
                .flags
                .iter()
                .find(|flag| *flag == "AccountBound" || *flag == "SoulbindOnAcquire")
                .is_some()
    }

    pub fn is_common_ascended_material(&self) -> bool {
        let name = &self.name;
        name == "Empyreal Fragment" || name == "Dragonite Ore" || name == "Pile of Bloodstone Dust"
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
    listings: i32,
    pub unit_price: i32,
    pub quantity: i32,
}

impl Listing {
    pub fn unit_price_minus_fees(&self) -> i32 {
        (self.unit_price as f32 * (1.0 - TRADING_POST_COMMISSION)).floor() as i32
    }
}

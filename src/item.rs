use std::fmt;

use serde::{Deserialize, Serialize};

use strum::Display;

use crate::config;
use crate::config::CONFIG;
use crate::money::Money;
use crate::api::ApiItem;

// types for /items
#[derive(Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: u32,
    pub name: String,
    #[serde(rename = "type")]
    item_type: Type,
    rarity: Rarity,
    level: i32,
    vendor_value: u32,
    flags: Vec<Flag>,
    restrictions: Vec<String>,
    upgrades_into: Option<Vec<Upgrade>>,
    upgrades_from: Option<Vec<Upgrade>>,
    details: Option<Details>,
}

impl From<ApiItem> for Item {
    fn from(item: ApiItem) -> Self {
        let details = match (&item.item_type, item.details) {
            (Type::Consumable, Some(details)) => Some(Details::Consumable(
                serde_json::from_value(details)
                    .unwrap_or_else(|err| panic!(
                        "Error parsing API consumable item: {}", err
                    )),
            )),
            _ => None,
        };
        Item {
            id: item.id,
            name: item.name,
            item_type: item.item_type,
            rarity: item.rarity,
            level: item.level,
            vendor_value: item.vendor_value,
            flags: item.flags,
            restrictions: item.restrictions,
            upgrades_into: item.upgrades_into,
            upgrades_from: item.upgrades_from,
            details: details,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Type {
    Armor,
    Back,
    Bag,
    Consumable,
    Container,
    CraftingMaterial,
    Gathering,
    Gizmo,
    Key,
    MiniPet,
    JadeTechModule,
    PowerCore,
    Tool,
    Trait,
    Trinket,
    Trophy,
    UpgradeComponent,
    Weapon,
    Mwcc, // TODO: From SoTO, will probably be renamed eventually
}

#[derive(Debug, Serialize, Deserialize, Display)]
pub enum Rarity {
    Junk,
    Basic,
    Fine,
    Masterwork,
    Rare,
    Exotic,
    Ascended,
    Legendary,
}

impl Rarity {
    fn crafted_localized(&self) -> String {
        let lang = CONFIG.lang.as_ref().unwrap_or(&config::Language::English);
        // NOTE: these strings were extracted by hand from client crafting interface
        match lang {
            config::Language::English => match &self {
                Self::Masterwork => "Master".to_string(),
                _ => self.to_string(),
            },
            config::Language::Spanish => match &self {
                Self::Masterwork => "maestro".to_string(),
                Self::Rare => "excepcional".to_string(),
                Self::Exotic => "exótico".to_string(),
                Self::Ascended => "Ascendido".to_string(),
                _ => self.to_string(),
            },
            config::Language::German => match &self {
                Self::Masterwork => "Meister".to_string(),
                Self::Rare => "Selten".to_string(),
                Self::Exotic => "Exotisch".to_string(),
                Self::Ascended => "Aufgestiegen".to_string(),
                _ => self.to_string(),
            },
            config::Language::French => match &self {
                Self::Masterwork => "Maître".to_string(),
                // Rare is the same in French
                Self::Exotic => "Exotique".to_string(),
                Self::Ascended => "Elevé".to_string(),
                _ => self.to_string(),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum Flag {
    AccountBindOnUse,
    AccountBound,
    Attuned,
    BulkConsume,
    DeleteWarning,
    HideSuffix,
    Infused,
    MonsterOnly,
    NoMysticForge,
    NoSalvage,
    NoSell,
    NotUpgradeable,
    NoUnderwater,
    SoulbindOnAcquire,
    SoulBindOnUse,
    Tonic,
    Unique,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Details {
    Consumable(ItemConsumableDetails),
    // don't care about the rest for now
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ItemConsumableDetails {
    #[serde(rename = "type")]
    consumable_type: ItemConsumableType,
    recipe_id: Option<u32>,
    extra_recipe_ids: Option<Vec<u32>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ItemConsumableType {
    AppearanceChange,
    Booze,
    ContractNpc,
    Currency,
    Food,
    Generic,
    Halloween,
    Immediate,
    MountRandomUnlock,
    RandomUnlock,
    Transmutation,
    Unlock,
    UpgradeRemoval,
    Utility,
    TeleportToFriend,
}

impl Item {
    // Output is cost per item, min purchase count
    pub fn vendor_cost(&self) -> Option<(Money, u32)> {
        // standard vendor sell price is generally buy price * 8, see:
        // https://forum-en.gw2archive.eu/forum/community/api/How-to-get-the-vendor-sell-price
        match &self.id {
            // Standard vendor sell price

            // Singular
            8576  | // Bottle of Rice Wine
            76839 | // Milling Basin
            70647 | // Crystalline Bottle
            75762 | // Bag of Mortar
            75087 | // Essence of Elegance
            // Rune of Holding: Minor, Regular, Major, Greater, Superior
            13010 | 13006 | 13007 | 13008 | 13009
                => Some((Money::from_copper((self.vendor_value * 8) as i32), 1)),
            // 10s
            12136 | // Bag of Flour - 1, from some vendors, 10 from master chefs
            19792 | // Spool of Jute Thread
            19789 | // Spool of Wool Thread
            19794 | // Spool of Cotton Thread
            19793 | // Spool of Linen Thread
            19791 | // Spool of Silk Thread
            19790 | // Spool of Gossamer Thread
            19704 | // Lump of Tin
            19750 | // Lump of Coal
            19924 | // Lump of Primordium
            12157 | // Jar of Vinegar
            12151 | // Packet of Baking Powder
            12158 | // Jar of Vegetable Oil
            12153 | // Packet of Salt
            12155 | // Bag of Sugar
            12156 | // Jug of Water
            12324 | // Bag of Starch
            12271   // Bottle of Soy Sauce
                // Price is already scaled per item
                => Some((Money::from_copper((self.vendor_value * 8) as i32), 10)),

            // Custom Price

            46747 => Some((Money::from_copper(1496) / 10, 10)), // Thermocatalytic Reagent
            91739 => Some((Money::from_copper(1496) / 10, 10)), // Pile of Compost Starter
            91702 => Some((Money::from_copper(1000) / 5, 5)), // Pile of Powdered Gelatin Mix; prereq achievement
            90201 => Some((Money::from_copper(40000), 1)), // Smell-Enhancing Culture; prereq achievement

            // Karma Ingredients - Bulk package item ids

            // Apples, Buttermilk, Celery Stalks, Cheese Wedges, Cumin, Green Beans, Lemons, Nutmeg
            // Seeds, Tomatoes, Yeast
            12788 | 12801 | 12790 | 12802 | 12793 | 12794 | 12795 | 12796 | 12798 | 12804
                if CONFIG.karma != None
                => Some((Money::from_karma(35), 1)),
            // Bananas, Basil Leaves, Bell Peppers, Black Beans, Kidney Beans, Rice
            12773 | 12774 | 12776 | 12777 | 12778 | 12780 if CONFIG.karma != None => Some((Money::from_karma(49), 1)),
            // Almonds, Avocados, Cherries, Ginger Root, Limes, Sour Cream
            12765 | 12766 | 12767 | 12768 | 12769 | 12764 if CONFIG.karma != None => Some((Money::from_karma(77), 1)),
            // Chickpeas, Coconuts, Horseradish Root, Pears, Pinenuts, Shallots
            12781 | 12782 | 12783 | 12785 | 12786 | 12787 if CONFIG.karma != None => Some((Money::from_karma(112), 1)),
            // Eggplants, Peaches
            12770 | 12771 if CONFIG.karma != None => Some((Money::from_karma(154), 1)),
            // Mangos
            12772 if CONFIG.karma != None => Some((Money::from_karma(203), 1)),

            _ => None,
        }
    }

    // Account Bound Tokens
    pub fn token_value(&self) -> Option<Money> {
        match &self.id {
            // Base game
            // Empyreal Fragment, Dragonite Ore, Pile of Bloodstone Dust
            46735 | 46733 | 46731 if CONFIG.ascended != None => Some(Money::from_copper(CONFIG.ascended.unwrap() as i32)),
            // LW1
            // 50025 Blade Shard
            50025 => Some(Money::from_copper(0)),
            // LW3
            // 79280 Blood Ruby
            // 79469 Petrified Wood
            // 80332 Jade Shard
            // 81127 Fire Orchid Blossom
            79280 | 79469 | 80332 | 81127 if CONFIG.um != None => Some(Money::from_um(38)),
            // 79899 Fresh Winterberry
            // 81706 Orrian Pearl
            79899 | 81706 if CONFIG.um != None => Some(Money::from_um(19)),
            // LW4
            // 86977 Difluorite Crystal
            // 87645 Inscribed Shard
            // 88955 Lump of Mistonium
            // 89537 Branded Mass
            // 90783 Mistborn Mote
            86069 | 86977 | 87645 | 88955 | 90783 if CONFIG.vm != None => Some(Money::from_vm(20)),
            // 86069 Kralkatite Ore
            89537 if CONFIG.vm != None => Some(Money::from_vm(4)),
            // Icebrood Saga
            // 92072 Hatched Chili
            92072 => Some(Money::from_copper(0)),
            // 92272 Eternal Ice Shard
            92272 if CONFIG.vm != None && CONFIG.karma != None => {
                // Can convert 75 into 10 tokens worth 20 VM each for 2688 karma
                let value = Money::new(0, -2688, 0, 200, 0) / 75;
                if value.to_copper_value() >= 0 {
                    Some(value)
                } else {
                    None
                }
            },

            // Currency items, for recording vendor purchases
            // TODO: like tokens, these aren't vendor items
            // Order is Basic / Fine / Mwk
            38030 if CONFIG.karma != None => Some(Money::from_karma(1)), // Drip of liquid karma;
                  // technically gives 150, but need 1
            79222 | 79061 | 79163 if CONFIG.um != None => Some(Money::from_um(5)), // UM;
                  // unspecified amount, but all purchases are divisible by 5, and consistent w/VM.
            86384 if CONFIG.vm != None => Some(Money::from_vm(1)), // VM; this item
                  // technically gives 5 VM. But not every VM purchase is divisible by 5.
            // 88926 - Provisioner Token
            96052 if CONFIG.rn != None => Some(Money::from_rn(1)), // Research Note

            _ => None,
        }
    }

    pub fn is_restricted(&self) -> bool {
        // 76363 == legacy catapult schematic
        self.id == 76363
            || self
                .flags
                .iter()
                .any(|flag| *flag == Flag::AccountBound || *flag == Flag::SoulbindOnAcquire)
    }

    pub fn recipe_unlocks(&self) -> Option<Vec<u32>> {
        match (&self.item_type, &self.details) {
            (Type::Consumable, Some(Details::Consumable(details))) => {
                let mut unlocks = vec![];
                if let Some(recipe_id) = details.recipe_id {
                    unlocks.push(recipe_id);
                } else if let Some(extra_recipe_ids) = &details.extra_recipe_ids {
                    unlocks.extend(extra_recipe_ids);
                }
                Some(unlocks)
            }
            (Type::Consumable, None) => {
                eprintln!("Item {} is a consumable with no details", self.id);
                None
            }
            _ => None,
        }
    }
}

// When printing an item, add rarity if a trinket, as most trinkets use the same
// name for different rarities
impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Type::Trinket = &self.item_type {
            write!(f, "{} ({})", &self.name, &self.rarity.crafted_localized())
        } else {
            write!(f, "{}", &self.name)
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Upgrade {
    upgrade: String,
    item_id: i32,
}

use crate::{
    api::{self, Item, ItemListings, Listing, RecipeIngredient},
    config::Discipline,
    crafting::{self, PurchasedIngredient, Recipe},
};

use num_rational::Rational32;

use std::collections::HashMap;

#[test]
fn calculate_crafting_profit_agony_infusion_unprofitable_test() {
    let data::TestData {
        item_id,
        items_map,
        recipes_map,
        tp_listings_map,
    } = data::agony_infusions();

    let profitable_item = crafting::calculate_crafting_profit(
        item_id,
        &recipes_map,
        &None,
        &items_map,
        &tp_listings_map,
        None,
        &Default::default(),
    );
    assert!(profitable_item.is_none());
}

#[test]
fn calculate_crafting_profit_agony_infusion_profitable_test() {
    let data::TestData {
        items_map,
        recipes_map,
        mut tp_listings_map,
        ..
    } = data::agony_infusions();

    let thermocatalytic_reagent_item_id = 46747;
    let plus_14_item_id = 49437;
    let plus_16_item_id = 49439;

    tp_listings_map
        .get_mut(&thermocatalytic_reagent_item_id)
        .unwrap()
        .sells
        .extend(
            vec![api::Listing {
                listings: 1,
                unit_price: 120,
                quantity: 1,
            }]
            .into_iter(),
        );
    tp_listings_map
        .get_mut(&plus_14_item_id)
        .unwrap()
        .sells
        .extend(
            vec![
                api::Listing {
                    listings: 7,
                    unit_price: 1100000,
                    quantity: 7,
                },
                api::Listing {
                    listings: 2,
                    unit_price: 1000000,
                    quantity: 2,
                },
                api::Listing {
                    listings: 1,
                    unit_price: 800000,
                    quantity: 1,
                },
            ]
            .into_iter(),
        );
    tp_listings_map.get_mut(&plus_16_item_id).unwrap().buys = vec![
        Listing {
            listings: 1,
            unit_price: 7982200,
            quantity: 1,
        },
        Listing {
            listings: 1,
            unit_price: 7982220,
            quantity: 1,
        },
    ];

    let mut purchased_ingredients = HashMap::new();
    let profitable_item = crafting::calculate_crafting_profit(
        plus_16_item_id,
        &recipes_map,
        &None,
        &items_map,
        &tp_listings_map,
        Some(&mut purchased_ingredients),
        &Default::default(),
    );

    let mut purchased_ingredients = purchased_ingredients.into_iter().collect::<Vec<_>>();
    purchased_ingredients.sort_by_key(|(key, _)| *key);
    assert_eq!(
        purchased_ingredients,
        vec![
            (
                (46747u32, crafting::Source::TradingPost),
                PurchasedIngredient {
                    count: 2.into(),
                    max_price: 178,
                    min_price: 120,
                    total_cost: 298.into()
                }
            ),
            (
                (46747u32, crafting::Source::Vendor),
                PurchasedIngredient {
                    count: 4.into(),
                    max_price: 0,
                    min_price: 0,
                    total_cost: 0.into()
                }
            ),
            (
                (49437u32, crafting::Source::TradingPost),
                PurchasedIngredient {
                    count: 8.into(),
                    max_price: 1100000,
                    min_price: 800000,
                    total_cost: 8300000.into()
                }
            ),
        ]
    );

    // NB: two reagents are purchased from the tp because that amount appears first in the recipe
    // and the average cost for two items is lower than from the vendor.
    // ideally only one reagent would be purchased from the tp, but that would introduce complexity.
    let thermocatalytic_reagent_crafting_cost = (120 + 178) + 4 * 150;
    let crafting_cost = 800000 + 2 * 1000000 + 5 * 1100000 + thermocatalytic_reagent_crafting_cost;
    assert_eq!(
        profitable_item,
        Some(crafting::ProfitableItem {
            id: plus_16_item_id,
            crafting_cost: crafting_cost.into(),
            crafting_steps: 6.into(),
            count: 2.into(),
            profit: api::subtract_trading_post_sales_commission(7982220 + 7982200)
                - Rational32::from(crafting_cost),
            unknown_recipes: Default::default(),
            max_sell: 7982220,
            min_sell: 7982200,
            breakeven: 5177000
        })
    );
}

#[test]
fn calculate_crafting_profit_with_output_item_count_test() {
    let item_id = 1236;

    let mut items_map = HashMap::new();
    items_map.insert(1234, Item::mock(1234, "Ingredient 2", 0));
    items_map.insert(1235, Item::mock(1235, "Ingredient 1", 0));
    items_map.insert(item_id, Item::mock(item_id, "Main item", 0));

    let mut recipes_map = HashMap::new();
    recipes_map.insert(
        item_id,
        Recipe::mock(
            7852,
            item_id,
            1,
            [],
            &[
                RecipeIngredient {
                    item_id: 1234,
                    count: 2,
                },
                RecipeIngredient {
                    item_id: 1235,
                    count: 1,
                },
            ],
        ),
    );

    let tp_listings_map = tp_listings_map(vec![
        (1234, vec![], vec![(94, 50), (92, 33), (90, 1)]),
        (1235, vec![], vec![(59, 50), (45, 33), (43, 1)]),
        (item_id, vec![(198, 47), (199, 50), (200, 1)], vec![]),
    ]);

    recipes_map.get_mut(&item_id).unwrap().output_item_count = 99;
    let profitable_item = crafting::calculate_crafting_profit(
        item_id,
        &recipes_map,
        &None,
        &items_map,
        &tp_listings_map,
        None,
        &Default::default(),
    );
    assert!(profitable_item.is_none());

    recipes_map.get_mut(&item_id).unwrap().output_item_count = 98;
    let profitable_item = crafting::calculate_crafting_profit(
        item_id,
        &recipes_map,
        &None,
        &items_map,
        &tp_listings_map,
        None,
        &Default::default(),
    );
    assert_eq!(
        profitable_item,
        Some(crafting::ProfitableItem {
            id: item_id,
            crafting_cost: (43 + 90 + 92).into(),
            crafting_steps: 1.into(),
            count: 98.into(),
            profit: api::subtract_trading_post_sales_commission(200 + 199 * 50 + 198 * 47)
                - Rational32::from(43 + 90 + 92),
            unknown_recipes: Default::default(),
            max_sell: 200,
            min_sell: 198,
            breakeven: 2
        })
    );

    recipes_map.get_mut(&item_id).unwrap().output_item_count = 3;
    let profitable_item = crafting::calculate_crafting_profit(
        item_id,
        &recipes_map,
        &None,
        &items_map,
        &tp_listings_map,
        None,
        &Default::default(),
    );
    let crafting_cost = 43 + 45 * 31 + 90 + 92 * 33 + 94 * 30;
    assert_eq!(
        profitable_item,
        Some(crafting::ProfitableItem {
            id: item_id,
            crafting_cost: crafting_cost.into(),
            crafting_steps: 32.into(),
            count: 96.into(),
            profit: api::subtract_trading_post_sales_commission(200 + 199 * 50 + 198 * 45)
                - Rational32::from(crafting_cost),
            unknown_recipes: Default::default(),
            max_sell: 200,
            min_sell: 198,
            breakeven: 91
        })
    );
}

fn tp_listings_map(
    from: Vec<(u32, Vec<(i32, i32)>, Vec<(i32, i32)>)>,
) -> HashMap<u32, ItemListings> {
    let mut map = HashMap::new();
    for (id, mut buys, mut sells) in from.into_iter() {
        buys.sort_by(|(price1, _), (price2, _)| price1.cmp(&price2));
        sells.sort_by(|(price1, _), (price2, _)| price1.cmp(&price2).reverse());
        map.insert(
            id,
            ItemListings {
                id,
                buys: buys
                    .into_iter()
                    .map(|(unit_price, quantity)| Listing {
                        listings: quantity,
                        unit_price,
                        quantity,
                    })
                    .collect(),
                sells: sells
                    .into_iter()
                    .map(|(unit_price, quantity)| Listing {
                        listings: quantity,
                        unit_price,
                        quantity,
                    })
                    .collect(),
            },
        );
    }
    map
}

mod data {
    use super::*;

    pub struct TestData {
        pub item_id: u32,
        pub items_map: HashMap<u32, Item>,
        pub recipes_map: HashMap<u32, Recipe>,
        pub tp_listings_map: HashMap<u32, ItemListings>,
    }

    /// Recipe with very large number of ingredients but low tp liquidity
    pub fn agony_infusions() -> TestData {
        let item_id = 49439;

        let mut items_map = HashMap::new();
        items_map.insert(46747, Item::mock(46747, "Thermocatalytic Reagent", 80));
        items_map.insert(49424, Item::mock(49424, "+1 Agony Infusion", 330));
        items_map.insert(49425, Item::mock(49425, "+2 Agony Infusion", 330));
        items_map.insert(49426, Item::mock(49426, "+3 Agony Infusion", 330));
        items_map.insert(49427, Item::mock(49427, "+4 Agony Infusion", 330));
        items_map.insert(49428, Item::mock(49428, "+5 Agony Infusion", 330));
        items_map.insert(49429, Item::mock(49429, "+6 Agony Infusion", 330));
        items_map.insert(49430, Item::mock(49430, "+7 Agony Infusion", 330));
        items_map.insert(49431, Item::mock(49431, "+8 Agony Infusion", 330));
        items_map.insert(49432, Item::mock(49432, "+9 Agony Infusion", 330));
        items_map.insert(49433, Item::mock(49433, "+10 Agony Infusion", 330));
        items_map.insert(49434, Item::mock(49434, "+11 Agony Infusion", 330));
        items_map.insert(49435, Item::mock(49435, "+12 Agony Infusion", 330));
        items_map.insert(49436, Item::mock(49436, "+13 Agony Infusion", 330));
        items_map.insert(49437, Item::mock(49437, "+14 Agony Infusion", 330));
        items_map.insert(49438, Item::mock(49438, "+15 Agony Infusion", 330));
        items_map.insert(49439, Item::mock(49439, "+16 Agony Infusion", 330));

        let mut recipes_map = HashMap::new();
        recipes_map.insert(
            49425,
            Recipe::mock(
                7851,
                49425,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49424,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49426,
            Recipe::mock(
                7852,
                49426,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49425,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49427,
            Recipe::mock(
                7853,
                49427,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49426,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49428,
            Recipe::mock(
                7854,
                49428,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49427,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49429,
            Recipe::mock(
                7855,
                49429,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49428,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49430,
            Recipe::mock(
                7856,
                49430,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49429,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49431,
            Recipe::mock(
                7857,
                49431,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49430,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49432,
            Recipe::mock(
                7858,
                49432,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49431,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49433,
            Recipe::mock(
                7859,
                49433,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49432,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49434,
            Recipe::mock(
                7860,
                49434,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49433,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49435,
            Recipe::mock(
                7861,
                49435,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49434,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49436,
            Recipe::mock(
                7862,
                49436,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49435,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49437,
            Recipe::mock(
                7863,
                49437,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49436,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49438,
            Recipe::mock(
                7864,
                49438,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49437,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );
        recipes_map.insert(
            49439,
            Recipe::mock(
                7865,
                49439,
                1,
                [Discipline::Artificer],
                &[
                    RecipeIngredient {
                        item_id: 49438,
                        count: 2,
                    },
                    RecipeIngredient {
                        item_id: 46747,
                        count: 1,
                    },
                ],
            ),
        );

        let mut tp_listings_map = HashMap::new();
        tp_listings_map.insert(
            46747,
            ItemListings {
                id: 46747,
                buys: [
                    Listing {
                        listings: 245,
                        unit_price: 147,
                        quantity: 59999,
                    },
                    Listing {
                        listings: 211,
                        unit_price: 148,
                        quantity: 50790,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 141,
                        unit_price: 179,
                        quantity: 33570,
                    },
                    Listing {
                        listings: 63,
                        unit_price: 178,
                        quantity: 15136,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49424,
            ItemListings {
                id: 49424,
                buys: [
                    Listing {
                        listings: 36,
                        unit_price: 73,
                        quantity: 8874,
                    },
                    Listing {
                        listings: 287,
                        unit_price: 74,
                        quantity: 71424,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 3,
                        unit_price: 81,
                        quantity: 553,
                    },
                    Listing {
                        listings: 2,
                        unit_price: 80,
                        quantity: 112,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49425,
            ItemListings {
                id: 49425,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 304,
                        quantity: 194,
                    },
                    Listing {
                        listings: 4,
                        unit_price: 305,
                        quantity: 1000,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 452,
                        quantity: 1,
                    },
                    Listing {
                        listings: 6,
                        unit_price: 451,
                        quantity: 1152,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49426,
            ItemListings {
                id: 49426,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 749,
                        quantity: 213,
                    },
                    Listing {
                        listings: 2,
                        unit_price: 751,
                        quantity: 355,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 3,
                        unit_price: 775,
                        quantity: 8,
                    },
                    Listing {
                        listings: 2,
                        unit_price: 774,
                        quantity: 5,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49427,
            ItemListings {
                id: 49427,
                buys: [
                    Listing {
                        listings: 2,
                        unit_price: 1937,
                        quantity: 319,
                    },
                    Listing {
                        listings: 7,
                        unit_price: 1950,
                        quantity: 1193,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 5,
                        unit_price: 2290,
                        quantity: 16,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 2100,
                        quantity: 12,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49428,
            ItemListings {
                id: 49428,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 3387,
                        quantity: 242,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 3500,
                        quantity: 10,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 2,
                        unit_price: 4495,
                        quantity: 6,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 4494,
                        quantity: 1,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49429,
            ItemListings {
                id: 49429,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 6500,
                        quantity: 16,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 6600,
                        quantity: 3,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 7082,
                        quantity: 1,
                    },
                    Listing {
                        listings: 2,
                        unit_price: 6333,
                        quantity: 2,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49430,
            ItemListings {
                id: 49430,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 14469,
                        quantity: 8,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 14475,
                        quantity: 27,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 17000,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 16996,
                        quantity: 1,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49431,
            ItemListings {
                id: 49431,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 30700,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 30707,
                        quantity: 3,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 2,
                        unit_price: 35900,
                        quantity: 11,
                    },
                    Listing {
                        listings: 2,
                        unit_price: 35897,
                        quantity: 4,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49432,
            ItemListings {
                id: 49432,
                buys: [
                    Listing {
                        listings: 2,
                        unit_price: 58033,
                        quantity: 87,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 58034,
                        quantity: 45,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 69000,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 68999,
                        quantity: 5,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49433,
            ItemListings {
                id: 49433,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 115306,
                        quantity: 3,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 115307,
                        quantity: 2,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 141500,
                        quantity: 3,
                    },
                    Listing {
                        listings: 2,
                        unit_price: 141300,
                        quantity: 2,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49434,
            ItemListings {
                id: 49434,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 235902,
                        quantity: 2,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 235903,
                        quantity: 3,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 298392,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 298390,
                        quantity: 1,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49435,
            ItemListings {
                id: 49435,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 454981,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 454982,
                        quantity: 2,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 585500,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 585499,
                        quantity: 1,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49436,
            ItemListings {
                id: 49436,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 944117,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 944118,
                        quantity: 1,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 1239994,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 1239990,
                        quantity: 1,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49437,
            ItemListings {
                id: 49437,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 1900958,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 1900960,
                        quantity: 4,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 2489189,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 2489188,
                        quantity: 1,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49438,
            ItemListings {
                id: 49438,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 3509999,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 3510000,
                        quantity: 1,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 4749997,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 4499999,
                        quantity: 1,
                    },
                ]
                .into(),
            },
        );
        tp_listings_map.insert(
            49439,
            ItemListings {
                id: 49439,
                buys: [
                    Listing {
                        listings: 1,
                        unit_price: 7982200,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 7982220,
                        quantity: 1,
                    },
                ]
                .into(),
                sells: [
                    Listing {
                        listings: 1,
                        unit_price: 9499998,
                        quantity: 1,
                    },
                    Listing {
                        listings: 1,
                        unit_price: 9499997,
                        quantity: 1,
                    },
                ]
                .into(),
            },
        );

        TestData {
            item_id,
            items_map,
            recipes_map,
            tp_listings_map,
        }
    }
}

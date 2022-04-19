use crate::api;
use crate::config;
use crate::gw2efficiency;

use std::convert::TryFrom;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

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

    pub fn is_automatic(&self) -> bool {
        match &self.source {
            RecipeSource::Purchasable | RecipeSource::Achievement => false,
            // These aren't included in the API; assume you know them
            RecipeSource::Automatic | RecipeSource::Discoverable => true,
            // TODO: instead, check if account has a char with the required crafting level
            // Would require a key with the characters scope. Still wouldn't detect
            // discoverable recipes, but would detect access to them
        }
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

    pub fn collect_ingredient_ids(
        &self,
        recipes_map: &HashMap<u32, Recipe>,
        ids: &mut Vec<u32>,
    ) {
        for ingredient in &self.ingredients {
            if ids.contains(&ingredient.item_id) {
                continue;
            }
            ids.push(ingredient.item_id);
            if let Some(recipe) = recipes_map.get(&ingredient.item_id) {
                recipe.collect_ingredient_ids(recipes_map, ids);
            }
        }
    }
}

pub fn mark_recursive_recipes(recipes_map: &HashMap<u32, Recipe>) -> HashSet<u32> {
    let mut set = HashSet::new();
    for (recipe_id, recipe) in recipes_map {
        mark_recursive_recipes_internal(
            *recipe_id,
            recipe.output_item_id,
            recipes_map,
            &mut vec![],
            &mut set,
        );
    }
    set
}

fn mark_recursive_recipes_internal(
    item_id: u32,
    search_output_item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    ingredients_stack: &mut Vec<u32>,
    set: &mut HashSet<u32>,
) {
    if set.contains(&item_id) {
        return;
    }
    if let Some(recipe) = recipes_map.get(&item_id) {
        for ingredient in &recipe.ingredients {
            if ingredient.item_id == search_output_item_id {
                set.insert(recipe.output_item_id);
                return;
            }
            // skip unnecessary recursion
            if ingredients_stack.contains(&ingredient.item_id) {
                continue;
            }
            ingredients_stack.push(ingredient.item_id);
            mark_recursive_recipes_internal(
                ingredient.item_id,
                search_output_item_id,
                recipes_map,
                ingredients_stack,
                set,
            );
            ingredients_stack.pop();
        }
    }
}

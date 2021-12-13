use serde::{de::Deserializer, Deserialize};
use serde_json::Value;

use crate::api;
use crate::crafting;
use crate::config;

use std::fs::File;
use std::path::Path;
use std::str::FromStr;

use bincode::{deserialize_from, serialize_into};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use phf::phf_set;

static RECIPE_BLACKLIST_IDS: phf::Set<u32> = phf_set! {
    // Non-integer output, probabalistic:
    19675_u32, // Mystic Clover
    38131_u32, // Delicate Snowflake
    38132_u32, // Glittering Snowflake
    38133_u32, // Unique Snowflake
    38134_u32, // Pristine Snowflake
    38135_u32, // Flawless Snowflake

    // Integer output is wrong, probabalistic:
    38121_u32, // Endless Gift Dolyak Tonic; 1/3 chance
    28115_u32, // Endless Toymaker's Tonic; 1/3 chance
    45008_u32, // Mini Steamrider; 1/3 chance
    45009_u32, // Mini Steam Hulk; 1/3 chance
    45010_u32, // Mini Steam Minotaur; 1/3 chance

    // Unclear if the halloween ingredients are random; but will probably never
    // be worth converting anyway, so leaving them off

    // Leaving in; at least lists the minimum accurately.
    // 68063_u32, // Amalgamated Gemstone; 10% chance of 25 from the 10 recipe, etc.
};

#[derive(Debug, Deserialize)]
pub struct Recipe {
    pub name: String, // used only in error output
    pub output_item_id: u32,
    #[serde(deserialize_with = "treat_error_as_none")]
    pub output_item_count: Option<i32>,
    #[serde(deserialize_with = "strum_discipline")]
    pub disciplines: Vec<config::Discipline>,
    pub ingredients: Vec<api::RecipeIngredient>,
}

pub async fn fetch_custom_recipes(
    cache_path: impl AsRef<Path>,
) -> Result<Vec<crafting::Recipe>, Box<dyn std::error::Error>> {
    if let Ok(file) = File::open(&cache_path) {
        let stream = DeflateDecoder::new(file);
        deserialize_from(stream).map_err(|e| {
            format!(
                "Failed to deserialize existing cache at '{}' ({}). \
                 Try using the --reset-cache flag to replace the cache file.",
                cache_path.as_ref().display(),
                e,
            )
            .into()
        })
    } else {
        let url = "https://raw.githubusercontent.com/gw2efficiency/custom-recipes/master/recipes.json";
        println!("Fetching {}", url);

        let custom_recipes: Vec<Recipe> = reqwest::get(url).await?.json().await?;

        let recipes: Vec<crafting::Recipe> = custom_recipes
            .into_iter()
            // Remove blacklisted recipes here to avoid printing errors for non-integers
            .filter(|r| !RECIPE_BLACKLIST_IDS.contains(&r.output_item_id))
            .map(std::convert::TryFrom::try_from)
            .filter_map(|result: Result<crafting::Recipe, _>| match result {
                Ok(recipe) => Some(recipe),
                Err(e) => {
                    eprintln!("{}", e);
                    None
                }
            })
            .collect();

        let file = File::create(cache_path)?;
        let stream = DeflateEncoder::new(file, Compression::default());
        serialize_into(stream, &recipes)?;

        Ok(recipes)
    }
}

fn treat_error_as_none<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    let value: Value = Deserialize::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

fn strum_discipline<'de, D>(deserializer: D) -> Result<Vec<config::Discipline>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value: Value = Deserialize::deserialize(deserializer)?;
    match value {
        Value::Array(vec) => {
            let mut c: Vec<config::Discipline> = Vec::new();
            for val in vec.iter() {
                if let Value::String(s) = val {
                    c.push(config::Discipline::from_str(s).map_err(|e| Error::custom(
                        format!("Unknown string \"{}\": {}", s, e)
                    ))?);
                } else {
                    return Err(Error::custom("Invalid discipline - not a string"));
                }
            }
            Ok(c)
        }
        _ => Err(Error::custom("Invalid discipline - not an array")),
    }
}

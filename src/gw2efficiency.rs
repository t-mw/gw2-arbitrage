use serde::{de::Deserializer, Deserialize};
use serde_json::Value;

use crate::api;
use crate::crafting;
use crate::config;

use std::fs::File;
use std::path::Path;

use bincode::{deserialize_from, serialize_into};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;

use std::str::FromStr;

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

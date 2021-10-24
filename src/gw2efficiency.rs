use serde::{de::Deserializer, Deserialize};
use serde_json::Value;

use crate::api;

#[derive(Debug, Deserialize)]
pub struct Recipe {
    pub name: String,
    pub output_item_id: u32,
    #[serde(deserialize_with = "treat_error_as_none")]
    pub output_item_count: Option<i32>,
    pub disciplines: Vec<String>,
    pub ingredients: Vec<api::RecipeIngredient>,
}

pub async fn fetch_custom_recipes() -> Result<Vec<Recipe>, Box<dyn std::error::Error>> {
    let url = "https://raw.githubusercontent.com/gw2efficiency/custom-recipes/master/recipes.json";
    println!("Fetching {}", url);
    Ok(reqwest::get(url).await?.json().await?)
}

fn treat_error_as_none<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    let value: Value = Deserialize::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

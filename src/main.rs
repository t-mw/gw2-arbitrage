use bincode::{deserialize_from, serialize_into};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

use std::fs::File;
use std::path::Path;

const MAX_PAGE_SIZE: u32 = 200;

#[derive(Debug, Serialize, Deserialize)]
struct Price {
    id: u32,
    buys: PriceInfo,
    sells: PriceInfo,
}

#[derive(Debug, Serialize, Deserialize)]
struct PriceInfo {
    unit_price: u32,
    quantity: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Recipe {
    id: u32,
    #[serde(rename = "type")]
    type_name: String,
    output_item_id: u32,
    output_item_count: u32,
    time_to_craft_ms: u32,
    disciplines: Vec<String>,
    min_rating: u32,
    flags: Vec<String>,
    ingredients: Vec<RecipeIngredient>,
    chat_link: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecipeIngredient {
    item_id: u32,
    count: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let current_exe = std::env::current_exe().unwrap();
    let current_exe_dir = current_exe.parent().unwrap();

    let prices_path = current_exe_dir.join("prices.bin");
    let recipes_path = current_exe_dir.join("recipes.bin");

    println!("Loading prices");
    let prices: Vec<Price> = ensure_paginated_cache(prices_path, "commerce/prices").await?;
    println!("Loaded {} prices", prices.len());

    println!("Loading recipes");
    let recipes: Vec<Recipe> = ensure_paginated_cache(recipes_path, "recipes").await?;
    println!("Loaded {} recipes", recipes.len());

    Ok(())
}

async fn ensure_paginated_cache<T>(
    cache_path: impl AsRef<Path>,
    url_path: &str,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    if let Ok(file) = File::open(&cache_path) {
        let stream = DeflateDecoder::new(file);
        deserialize_from(stream).map_err(|e| e.into())
    } else {
        let mut items: Vec<T> = vec![];

        let mut page_no: u32 = 0;
        let mut page_total: u32 = 1;

        while page_no < page_total {
            let url = format!(
                "https://api.guildwars2.com/v2/{}?page={}&page_size={}",
                url_path, page_no, MAX_PAGE_SIZE
            );

            println!("Fetching {}", url);
            let response = reqwest::get(&url).await?;

            let page_total_str = response
                .headers()
                .get("X-Page-Total")
                .expect("Missing X-Page-Total header")
                .to_str()
                .expect("X-Page-Total header contains invalid string");
            page_total = page_total_str.parse().unwrap_or_else(|_| {
                panic!("X-Page-Total is an invalid integer: {}", page_total_str)
            });

            items.append(&mut response.json::<Vec<T>>().await?);

            page_no += 1;
        }

        let file = File::create(cache_path)?;
        let stream = DeflateEncoder::new(file, Compression::default());
        serialize_into(stream, &items)?;

        Ok(items)
    }
}

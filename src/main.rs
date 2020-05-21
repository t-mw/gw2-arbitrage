use bincode::{deserialize_from, serialize_into};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

use std::fs::File;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let current_exe = std::env::current_exe().unwrap();
    let current_exe_dir = current_exe.parent().unwrap();

    let prices_path = current_exe_dir.join("prices.bin");

    let prices = if let Ok(file) = File::open(&prices_path) {
        println!("Loading cached prices");

        let stream = DeflateDecoder::new(file);
        deserialize_from(stream)?
    } else {
        let mut prices: Vec<Price> = vec![];

        let mut page_no: u32 = 0;
        let mut page_total: u32 = 1;

        while page_no < page_total {
            let url = format!(
                "https://api.guildwars2.com/v2/commerce/prices?page={}&page_size={}",
                page_no, MAX_PAGE_SIZE
            );

            println!("Fetching prices from {}", url);
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

            prices.append(&mut response.json::<Vec<Price>>().await?);

            page_no += 1;
        }

        let file = File::create(prices_path)?;
        let stream = DeflateEncoder::new(file, Compression::default());
        serialize_into(stream, &prices)?;

        prices
    };

    println!("Loaded {} prices", prices.len());

    Ok(())
}

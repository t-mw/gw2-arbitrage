use crate::api::ItemListings;

use bincode;
use bincode::{deserialize_from, serialize_into};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use futures::{stream, StreamExt};
use serde_json;

use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::config;

const PARALLEL_REQUESTS: usize = 10;
const MAX_PAGE_SIZE: i32 = 200; // https://wiki.guildwars2.com/wiki/API:2#Paging
const MAX_ITEM_ID_LENGTH: i32 = 200; // error returned for greater than this amount

pub async fn fetch_item_listings(
    item_ids: &[u32],
    cache_dir: Option<&PathBuf>,
) -> Result<Vec<ItemListings>, Box<dyn std::error::Error>> {
    let mut tp_listings: Vec<ItemListings> =
        request_item_ids("commerce/listings", item_ids, cache_dir).await?;

    for listings in &mut tp_listings {
        // by default sells are listed in ascending and buys in descending price.
        // reverse lists to allow best offers to be popped instead of spliced from front.
        listings.buys.reverse();
        listings.sells.reverse();
    }

    Ok(tp_listings)
}

pub async fn ensure_paginated_cache<T>(
    cache_path: impl AsRef<Path>,
    url_path: &str,
    lang: &Option<config::Language>,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
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
        let items = request_paginated(url_path, lang).await?;

        let file = File::create(cache_path)?;
        let stream = DeflateEncoder::new(file, Compression::default());
        serialize_into(stream, &items)?;

        Ok(items)
    }
}

pub async fn request_paginated<T>(
    url_path: &str,
    lang: &Option<config::Language>,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    let mut page_no = 0;
    let mut page_total = None;

    // update page total with first request
    let mut items: Vec<T> = request_page(url_path, page_no, &mut page_total, lang).await?;

    // fetch remaining pages in parallel batches
    page_no += 1;

    // try fetching one extra page in case page total increased while paginating
    let page_total = page_total.expect("Missing page total") + 1;

    let request_results = stream::iter((page_no..page_total).map(|page_no| async move {
        request_page::<T>(url_path, page_no, &mut Some(page_total), lang).await
    }))
    .buffered(PARALLEL_REQUESTS)
    .collect::<Vec<Result<Vec<T>, Box<dyn std::error::Error>>>>()
    .await;

    for result in request_results.into_iter() {
        let mut new_items = result?;
        items.append(&mut new_items);
    }

    Ok(items)
}

async fn request_page<T>(
    url_path: &str,
    page_no: usize,
    page_total: &mut Option<usize>,
    lang: &Option<config::Language>,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    let url = if let Some(code) = config::Language::code(lang) {
        format!(
            "https://api.guildwars2.com/v2/{}?lang={}&page={}&page_size={}",
            url_path, code, page_no, MAX_PAGE_SIZE
        )
    } else {
        format!(
            "https://api.guildwars2.com/v2/{}?page={}&page_size={}",
            url_path, page_no, MAX_PAGE_SIZE
        )
    };

    println!("Fetching {}", url);
    let response = reqwest::get(&url).await?;

    if page_total.is_none() {
        let page_total_str = response
            .headers()
            .get("X-Page-Total")
            .expect("Missing X-Page-Total header")
            .to_str()
            .expect("X-Page-Total header contains invalid string");
        *page_total =
            Some(page_total_str.parse().unwrap_or_else(|_| {
                panic!("X-Page-Total is an invalid integer: {}", page_total_str)
            }));
    }

    let txt = response.text().await?;
    if txt.contains("page out of range") {
        return Ok(vec![]);
    }
    let de = &mut serde_json::Deserializer::from_str(&txt);
    serde_path_to_error::deserialize(de).map_err(|e| e.into())
}

async fn request_item_ids<T>(
    url_path: &str,
    item_ids: &[u32],
    cache_dir: Option<&PathBuf>,
) -> Result<Vec<T>, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    let mut result = vec![];

    for batch in item_ids.chunks(MAX_ITEM_ID_LENGTH as usize) {
        let item_ids_str: Vec<String> = batch.iter().map(|id| id.to_string()).collect();

        let url = format!(
            "https://api.guildwars2.com/v2/{}?ids={}",
            url_path,
            item_ids_str.join(",")
        );
        if let Some(cache_dir) = cache_dir {
            result.extend(cache_get::<Vec<T>>(&url, cache_dir, None).await?.into_iter());
        } else {
            result.extend(fetch::<Vec<T>>(&url, None).await?.into_iter());
        }
    }

    Ok(result)
}

pub async fn fetch_account_recipes(key: &str, cache_dir: &PathBuf) -> Result<HashSet<u32>, Box<dyn std::error::Error>> {
    let base = "https://api.guildwars2.com/v2/account/recipes?access_token=";
    let url = format!("{}{}", base, key);
    let display = format!("{}{}", base, "<api-key>");
    Ok(cache_get(&url, cache_dir, Some(&display)).await?)
}

async fn cache_get<T>(
    url: &str,
    cache_dir: &PathBuf,
    display: Option<&str>,
) -> Result<T, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    // Hash URL, check if cached, deserialize if so
    // We're hashing on url since the API does as well
    let mut hash = DefaultHasher::new();
    url.hash(&mut hash);
    let hash = hash.finish();

    let mut cache_file = cache_dir.clone();
    cache_file.push(format!("{}{}", config::CACHE_PREFIX, hash));

    if let Ok(file) = File::open(&cache_file) {
        let stream = DeflateDecoder::new(file);
        let v = deserialize_from(stream)?;

        return Ok(v)
    }

    let v = fetch(&url, display).await?;

    // save cache file
    let file = File::create(cache_file)?;
    let stream = DeflateEncoder::new(file, Compression::default());
    serialize_into(stream, &v)?;

    Ok(v)
}

async fn fetch<T>(
    url: &str,
    display: Option<&str>,
) -> Result<T, Box<dyn std::error::Error>>
where
    T: serde::Serialize,
    T: serde::de::DeserializeOwned,
{
    if let Some(url) = display {
        println!("Fetching {}", url);
    } else {
        println!("Fetching {}", url);
    }

    let response = reqwest::get(url).await?;
    let status = response.status();
    if !status.is_success() {
        let err: serde_json::value::Value = response.json().await?;
        let text = err
            .get("text")
            .and_then(|text| text.as_str())
            .unwrap_or_else(|| status.as_str());
        return Err(text.into());
    }

    let bytes = response.bytes().await?;
    let de = &mut serde_json::Deserializer::from_slice(&bytes);
    let v: T = serde_path_to_error::deserialize(de)?;

    Ok(v)
}

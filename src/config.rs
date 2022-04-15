use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use std::collections::HashSet;
use num_rational::Rational32;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use strum::{Display, EnumString, EnumVariantNames, VariantNames};
use toml;

use lazy_static::lazy_static;

pub const CACHE_PREFIX: &str = "cache_";

#[derive(Debug, Default)]
pub struct CraftingOptions {
    pub include_timegated: bool,
    pub count: Option<u32>,
    pub threshold: Option<u32>,
    pub value: Option<u32>,
}

#[derive(Default)]
pub struct Config {
    pub crafting: CraftingOptions,

    pub output_csv: Option<PathBuf>,
    pub filter_disciplines: Option<Vec<Discipline>>,
    pub lang: Option<Language>,
    pub api_key: Option<String>,

    // Currency conversion values
    pub ascended: Option<u32>,
    pub karma: Option<Rational32>,
    pub um: Option<Rational32>,
    pub vm: Option<Rational32>,

    pub cache_dir: PathBuf,
    pub api_recipes_file: PathBuf,
    pub custom_recipes_file: PathBuf,
    pub items_file: PathBuf,

    pub item_blacklist: Option<HashSet<u32>>,
    pub recipe_blacklist: Option<HashSet<u32>>,

    pub item_id: Option<u32>,
}

lazy_static! {
    pub static ref CONFIG: Config = Config::new();
}

impl Config {
    fn new() -> Self {
        let mut config = Config::default();

        let opt = Opt::from_args();

        config.crafting.include_timegated = opt.include_timegated;
        config.crafting.count = opt.count;
        config.crafting.threshold = opt.threshold;
        config.crafting.value = opt.value;

        config.output_csv = opt.output_csv;

        config.item_id = opt.item_id;

        config.filter_disciplines = opt.filter_disciplines;

        let file: ConfigFile = match get_file_config(&opt.config_file) {
            Ok(config) => config,
            Err(e) => {
                println!("Error opening config file: {}", e);
                ConfigFile::default()
            }
        };

        config.api_key = file.api_key;

        config.lang = if let Some(_) = opt.lang {
            opt.lang
        } else if let Some(code) = file.lang {
            code.parse().map_or_else(
                |e| {
                    println!("Config file: {}", e);
                    None
                },
                |c| Some(c),
            )
        } else {
            None
        };

        config.ascended = if let Some(provided) = opt.ascended_value {
            if let Some(value) = provided {
                Some(value)
            } else {
                Some(0)
            }
        } else if let Some(currencies) = &file.currencies {
            if let Some(value) = currencies.ascended {
                Some(value)
            } else {
                None
            }
        } else {
            None
        };

        config.karma = if let Some(value) = opt.karma {
            Rational32::approximate_float(value)
        } else if let Some(currencies) = &file.currencies {
            if let Some(value) = currencies.karma {
                Rational32::approximate_float(value)
            } else {
                None
            }
        } else {
            None
        };

        config.um = if let Some(value) = opt.um {
            Rational32::approximate_float(value)
        } else if let Some(currencies) = &file.currencies {
            if let Some(value) = currencies.um {
                Rational32::approximate_float(value)
            } else {
                None
            }
        } else {
            None
        };

        config.vm = if let Some(value) = opt.vm {
            Rational32::approximate_float(value)
        } else if let Some(currencies) = &file.currencies {
            if let Some(value) = currencies.vm {
                Rational32::approximate_float(value)
            } else {
                None
            }
        } else {
            None
        };

        config.item_blacklist = if let Some(blacklist_section) = &file.blacklist {
            if let Some(items) = &blacklist_section.items {
                let mut set = HashSet::new();
                for item in items {
                    set.insert(*item);
                }
                Some(set)
            } else {
                None
            }
        } else {
            None
        };
        config.recipe_blacklist = if let Some(blacklist_section) = &file.blacklist {
            if let Some(recipes) = &blacklist_section.recipes {
                let mut set = HashSet::new();
                for recipe in recipes {
                    set.insert(*recipe);
                }
                Some(set)
            } else {
                None
            }
        } else {
            None
        };

        let cache_dir = cache_dir(&opt.cache_dir).expect("Failed to identify cache dir");
        ensure_dir(&cache_dir).expect("Failed to create cache dir");
        match flush_cache(&cache_dir) {
            Err(e) => println!("Failed to flush cache dir {}: {}", &cache_dir.display(), e),
            _ => (),
        }
        config.cache_dir = cache_dir;

        let data_dir = data_dir(&opt.data_dir).expect("Failed to identify data dir");
        ensure_dir(&data_dir).expect("Failed to create data dir");

        let mut api_recipes_path = data_dir.clone();
        api_recipes_path.push("recipes.bin");
        config.api_recipes_file = api_recipes_path;

        let mut custom_recipes_path = data_dir.clone();
        custom_recipes_path.push("custom.bin");
        config.custom_recipes_file = custom_recipes_path;

        let lang_suffix =
            Language::code(&config.lang).map_or_else(|| "".to_string(), |c| format!("_{}", c));
        let mut items_path = data_dir.clone();
        items_path.push(format!("items{}.bin", lang_suffix));
        config.items_file = items_path;

        if opt.reset_data {
            match remove_data_file(&config.items_file) {
                Err(e) => println!(
                    "Failed to remove file {}: {}",
                    &config.items_file.display(),
                    e
                ),
                _ => (),
            };
            match remove_data_file(&config.api_recipes_file) {
                Err(e) => println!(
                    "Failed to remove file {}: {}",
                    &config.api_recipes_file.display(),
                    e
                ),
                _ => (),
            };
            match remove_data_file(&config.custom_recipes_file) {
                Err(e) => println!(
                    "Failed to remove file {}: {}",
                    &config.custom_recipes_file.display(),
                    e
                ),
                _ => (),
            };
        }

        config
    }
}

fn get_file_config(file: &Option<PathBuf>) -> Result<ConfigFile, Box<dyn std::error::Error>> {
    let mut file = File::open(config_file(file)?)?;
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    Ok(toml::from_str(&s)?)
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    // API key requires scope unlocks
    api_key: Option<String>,
    lang: Option<String>,
    currencies: Option<ConfigFileCurrencySection>,
    blacklist: Option<ConfigFileBlacklistSection>,
}
#[derive(Debug, Default, Deserialize)]
struct ConfigFileCurrencySection {
    ascended: Option<u32>,
    karma: Option<f64>,
    um: Option<f64>,
    vm: Option<f64>,
}
#[derive(Debug, Default, Deserialize)]
struct ConfigFileBlacklistSection {
    items: Option<Vec<u32>>,
    recipes: Option<Vec<u32>>,
}

#[derive(StructOpt, Debug)]
struct Opt {
    /// Include timegated recipes such as Deldrimor Steel Ingot
    #[structopt(short = "t", long)]
    include_timegated: bool,

    /// Output the full list of profitable recipes to this CSV file
    #[structopt(short, long, parse(from_os_str))]
    output_csv: Option<PathBuf>,

    /// Print a shopping list of ingredients for the given item id
    item_id: Option<u32>,

    /// Limit the maximum number of items produced for a recipe
    #[structopt(short, long)]
    count: Option<u32>,

    /// Calculate profit based on a fixed value instead of from buy orders
    #[structopt(long)]
    value: Option<u32>,

    /// Threshold - min profit per item in copper
    #[structopt(long)]
    threshold: Option<u32>,

    #[structopt(short = "d", long = "disciplines", use_delimiter = true, help = &DISCIPLINES_HELP, parse(try_from_str = get_discipline))]
    filter_disciplines: Option<Vec<Discipline>>,

    /// Download recipes and items from the GW2 API, replacing any previously cached recipes and items
    #[structopt(long)]
    reset_data: bool,

    #[structopt(long, parse(from_os_str), help = &CACHE_DIR_HELP)]
    cache_dir: Option<PathBuf>,

    #[structopt(long, parse(from_os_str), help = &DATA_DIR_HELP)]
    data_dir: Option<PathBuf>,

    #[structopt(long, parse(from_os_str), help = &CONFIG_FILE_HELP)]
    config_file: Option<PathBuf>,

    /// One of "en", "es", "de", or "fr". Defaults to "en"
    // /// One of "en", "es", "de", "fr", or "zh". Defaults to "en"
    #[structopt(long, parse(try_from_str = get_lang))]
    lang: Option<Language>,

    /// Include recipes that require Piles of Bloodstone Dust, Dragonite Ore or Empyreal Fragments,
    /// with an optional opportunity cost per item
    #[structopt(short = "a", long)]
    ascended_value: Option<Option<u32>>,

    /// Include recipes that require ingredients that can only be purchased with karma, using this
    /// conversion factor as the opportunity cost
    #[structopt(long)]
    karma: Option<f64>,

    /// Include recipes that use LW3 map tokens, using this conversion factor as the opportunity cost
    #[structopt(long)]
    um: Option<f64>,

    /// Include recipes that use LW4 map tokens, using this conversion factor as the opportunity cost
    #[structopt(long)]
    vm: Option<f64>,
}

static CACHE_DIR_HELP: Lazy<String> = Lazy::new(|| {
    format!(
        r#"Save cached API calls to this directory

If provided, the parent directory of the cache directory must already exist. Defaults to '{}'."#,
        cache_dir(&None).unwrap().display()
    )
});

static DATA_DIR_HELP: Lazy<String> = Lazy::new(|| {
    format!(
        r#"Save cached recipes and items to this directory

If provided, the parent directory of the cache directory must already exist. Defaults to '{}'."#,
        data_dir(&None).unwrap().display()
    )
});

static CONFIG_FILE_HELP: Lazy<String> = Lazy::new(|| {
    format!(
        r#"Read config options from this file. Supported options:

    api_key = "<key-with-unlocks-scope>"
    lang = "<lang>"

    [currencies]
    ascended = <opportunity cost per item>
    karma = <opportunity cost per karma>
    um = <opportunity cost per um>
    vm = <opportunity cost per vm>

The default file location is '{}'."#,
        config_file(&None).unwrap().display()
    )
});

static DISCIPLINES_HELP: Lazy<String> = Lazy::new(|| {
    format!(
        r#"Only show items craftable by this discipline or comma-separated list of disciplines (e.g. -d=Weaponsmith,Armorsmith)

valid values: {}"#,
        Discipline::VARIANTS.join(", ")
    )
});

#[derive(Debug, EnumString, EnumVariantNames)]
pub enum Language {
    #[strum(serialize = "en")]
    English,
    #[strum(serialize = "es")]
    Spanish,
    #[strum(serialize = "de")]
    German,
    #[strum(serialize = "fr")]
    French,
    // If you read this and can help test the TP code and extract strings from the Chinese version,
    // and would like to see this work in Chinese, please open an issue.
    // #[strum(serialize="zh")]
    // Chinese, // No lang client, and TP might have different data source anyway
}
impl Language {
    pub fn code(lang: &Option<Language>) -> Option<&str> {
        if let Some(lang) = lang {
            match lang {
                Language::English => None, // English is the default, so leave it off
                Language::Spanish => Some("es"),
                Language::German => Some("de"),
                Language::French => Some("fr"),
                //Language::Chinese => Some("zh"),
            }
        } else {
            None
        }
    }
}

fn get_lang<Language: FromStr + VariantNames>(
    code: &str,
) -> Result<Language, Box<dyn std::error::Error>> {
    Language::from_str(code).map_err(|_| {
        format!(
            "Invalid language: {} (valid values are {})",
            code,
            Language::VARIANTS.join(", ")
        )
        .into()
    })
}

#[derive(
    Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Display, EnumString, EnumVariantNames,
)]
pub enum Discipline {
    // TODO: swap these next two, next time rebuilding data files, so alphabetical
    Artificer,
    Armorsmith,
    Chef,
    Huntsman,
    Jeweler,
    Leatherworker,
    Tailor,
    Weaponsmith,
    Scribe,
    // A few more for compatibility with gw2efficiency
    #[strum(serialize = "Mystic Forge")]
    MysticForge,
    #[strum(serialize = "Double Click")]
    DoubleClick,
    Salvage,
    Merchant,
    Charge,
    Achievement,
    Growing,
}

fn get_discipline<Discipline: FromStr + VariantNames>(
    discipline: &str,
) -> Result<Discipline, Box<dyn std::error::Error>> {
    Discipline::from_str(discipline).map_err(|_| {
        format!(
            "Invalid discipline: {} (valid values are {})",
            discipline,
            Discipline::VARIANTS.join(", ")
        )
        .into()
    })
}

fn ensure_dir(dir: &PathBuf) -> Result<&PathBuf, Box<dyn std::error::Error>> {
    if !dir.exists() {
        std::fs::create_dir(&dir)
            .map_err(|e| format!("Failed to create '{}' ({})", dir.display(), e).into())
            .and(Ok(dir))
    } else {
        Ok(dir)
    }
}

fn cache_dir(dir: &Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(dir) = dir {
        return Ok(dir.clone());
    }
    dirs::cache_dir()
        .filter(|d| d.exists())
        .map(|mut cache_dir| {
            cache_dir.push("gw2-arbitrage");
            cache_dir
        })
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "Failed to access current working directory".into())
}

fn flush_cache(cache_dir: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    // flush any cache files older than 5 mins - which is how long the API caches url results.
    // Assume our request triggered the cache
    // Give a prefix; on Windows the user cache and user local data folders are the same
    let expired = SystemTime::now() - Duration::new(300, 0);
    for file in fs::read_dir(&cache_dir)? {
        let file = file?;
        let filename = file.file_name().into_string();
        if let Ok(name) = filename {
            if !name.starts_with(CACHE_PREFIX) {
                continue;
            }
        }
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        if metadata.created()? <= expired {
            fs::remove_file(file.path())?;
        }
    }
    Ok(())
}

fn data_dir(dir: &Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(dir) = dir {
        return Ok(dir.clone());
    }
    dirs::data_dir()
        .filter(|d| d.exists())
        .map(|mut data_dir| {
            data_dir.push("gw2-arbitrage");
            data_dir
        })
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "Failed to access current working directory".into())
}

fn remove_data_file(file: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    if file.exists() {
        println!("Removing existing data file at '{}'", file.display());
        std::fs::remove_file(&file)
            .map_err(|e| format!("Failed to remove '{}' ({})", file.display(), e))?;
    }
    Ok(())
}

fn config_file(file: &Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(file) = file {
        return Ok(file.clone());
    }
    dirs::config_dir()
        .filter(|d| d.exists())
        .map(|mut config_dir| {
            config_dir.push("gw2-arbitrage");
            config_dir
        })
        .or_else(|| std::env::current_dir().ok())
        .and_then(|mut path| {
            path.push("gw2-arbitrage.toml");
            Some(path)
        })
        .ok_or_else(|| "Failed to access current working directory".into())
}

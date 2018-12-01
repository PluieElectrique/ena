//! Configuration file parsing.

use std::fs::{self, File};
use std::io::prelude::*;
use std::io::BufReader;
use std::path::PathBuf;
use std::time::Duration;

use failure::ResultExt;
use serde::de::Error;
use serde::{Deserialize, Deserializer};

use four_chan::Board;

/// The main configuration file struct.
#[derive(Deserialize)]
pub struct Config {
    pub scraping: ScrapingConfig,
    pub network: NetworkConfig,
    pub database_media: DatabaseMediaConfig,
    pub asagi_compat: AsagiCompatibilityConfig,
}

/// A struct for scraping configuration.
#[derive(Deserialize)]
pub struct ScrapingConfig {
    pub boards: Vec<Board>,
    #[serde(deserialize_with = "nonzero_duration_from_secs")]
    pub poll_interval: Duration,
    pub fetch_archive: bool,
    pub download_media: bool,
    pub download_thumbs: bool,
}

/// The struct for network (general API request settings) configuration.
#[derive(Deserialize)]
pub struct NetworkConfig {
    pub rate_limiting: RateLimitingConfig,
    pub retry_backoff: RetryBackoffConfig,
}

/// The struct for the rate limiting configuration section.
#[derive(Deserialize)]
pub struct RateLimitingConfig {
    pub media: RateLimitingSettings,
    pub thread: RateLimitingSettings,
    pub thread_list: RateLimitingSettings,
}

/// A struct for individual rate limiting settings.
#[derive(Deserialize)]
pub struct RateLimitingSettings {
    #[serde(deserialize_with = "nonzero_duration_from_secs")]
    pub interval: Duration,
    #[serde(deserialize_with = "validate_max_interval")]
    pub max_interval: usize,
    #[serde(deserialize_with = "validate_max_concurrent")]
    pub max_concurrent: usize,
}

/// The struct for configuration of the exponential backoff for request retrying.
#[derive(Clone, Copy, Deserialize)]
pub struct RetryBackoffConfig {
    #[serde(deserialize_with = "nonzero_duration_from_secs")]
    pub base: Duration,
    pub factor: u32,
    #[serde(deserialize_with = "duration_from_secs")]
    pub max: Duration,
}

/// A struct for database and media directory configuration.
#[derive(Deserialize)]
pub struct DatabaseMediaConfig {
    pub database_url: String,
    pub charset: String,
    #[serde(deserialize_with = "validate_media_path")]
    pub media_path: PathBuf,
}

/// A struct for Asagi compatibility configuration.
#[derive(Deserialize)]
pub struct AsagiCompatibilityConfig {
    pub adjust_timestamps: bool,
    pub refetch_archived_threads: bool,
    pub always_add_archive_times: bool,
    pub create_index_counters: bool,
}

/// Configuration parsing errors.
#[derive(Debug, Fail)]
pub enum ConfigError {
    #[fail(display = "`database_media.charset` must not be empty")]
    EmptyCharset,

    #[fail(display = "`database_media.database_url` must not be empty")]
    EmptyDatabaseUrl,

    #[fail(display = "`scraping.boards` must contain at least one board to scrape")]
    NoBoards,

    #[fail(display = "`network.retry_backoff.factor` must be at least 2")]
    ZeroRetryFactor,
}

/// Read the configuration file `ena.toml` and parse it.
pub fn parse_config() -> Result<Config, failure::Error> {
    let file = File::open("ena.toml").context("Could not open ena.toml")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader
        .read_to_string(&mut contents)
        .context("Could not read ena.toml")?;

    let mut config: Config = toml::from_str(&contents).context("Could not parse ena.toml")?;
    config.scraping.boards.sort();
    config.scraping.boards.dedup();

    if config.scraping.boards.is_empty() {
        return Err(ConfigError::NoBoards.into());
    } else if config.database_media.charset.is_empty() {
        return Err(ConfigError::EmptyCharset.into());
    } else if config.database_media.database_url.is_empty() {
        return Err(ConfigError::EmptyDatabaseUrl.into());
    } else if config.network.retry_backoff.factor < 2 {
        return Err(ConfigError::ZeroRetryFactor.into());
    }

    fs::create_dir_all(&config.database_media.media_path)
        .context("Could not create media directory")?;
    let mut test_file = config.database_media.media_path.clone();
    test_file.push("ena_permission_test");
    File::create(&test_file).context("Could not create test file in media directory")?;
    fs::remove_file(&test_file).context("Could not remove media directory permission test file")?;

    if config.scraping.poll_interval.as_secs() < 10 {
        warn!("4chan API rules recommend a minimum `poll_interval` of 10 seconds");
        warn!("A very short `poll_interval` may cause the API to return old data");
    }

    Ok(config)
}

macro_rules! deserialize_validate {
    ($name:ident, $from:ty => $to:ty, $pred:expr, $ok:expr, $err:expr $(,)*) => {
        fn $name<'de, D>(deserializer: D) -> Result<$to, D::Error>
        where
            D: Deserializer<'de>,
        {
            let data: $from = Deserialize::deserialize(deserializer)?;
            if $pred(&data) {
                Ok($ok(data))
            } else {
                Err(D::Error::custom($err))
            }
        }
    };
}

deserialize_validate!(
    duration_from_secs,
    u64 => Duration,
    |_| true,
    |secs| Duration::from_secs(secs),
    "",
);

deserialize_validate!(
    nonzero_duration_from_secs,
    u64 => Duration,
    |&secs| secs != 0,
    |secs| Duration::from_secs(secs),
    "interval must be at least 1 second",
);

deserialize_validate!(
    validate_max_interval,
    usize => usize,
    |&max| max != 0,
    |max| max,
    "`max_interval` must be at least 1",
);

deserialize_validate!(
    validate_max_concurrent,
    usize => usize,
    |&max| max != 0,
    |max| max,
    "`max_concurrent` must be at least 1",
);

deserialize_validate!(
    validate_media_path,
    String => PathBuf,
    |s: &str| !s.is_empty(),
    |s| PathBuf::from(s),
    "media path must not be empty (use \".\" for current directory)",
);

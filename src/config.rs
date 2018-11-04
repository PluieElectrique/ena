//! Configuration file parsing.

use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;
use std::path::PathBuf;

use failure::{Error, ResultExt};

use four_chan::Board;

/// The main configuration file struct.
#[derive(Deserialize)]
pub struct Config {
    pub scraping: ScrapingConfig,
    pub rate_limiting: RateLimitingConfig,
    pub database_media: DatabaseMediaConfig,
    pub asagi_compat: AsagiCompatibilityConfig,
}

/// A struct for scraping configuration.
#[derive(Deserialize)]
pub struct ScrapingConfig {
    pub boards: Vec<Board>,
    pub poll_interval: u64,
    pub fetch_archive: bool,
    pub download_media: bool,
    pub download_thumbs: bool,
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
    pub interval: u64,
    pub max_interval: usize,
    pub max_concurrent: usize,
}

/// A struct for database and media directory configuration.
#[derive(Deserialize)]
pub struct DatabaseMediaConfig {
    pub database_url: String,
    pub charset: String,
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
    #[fail(display = "`poll_interval` must be at least 1 second (preferably 10 seconds or more)")]
    ZeroPollInterval,
}

/// Read the configuration file `ena.toml` and parse it.
pub fn parse_config() -> Result<Config, Error> {
    let file = File::open("ena.toml").context("Could not open ena.toml")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader
        .read_to_string(&mut contents)
        .context("Could not read ena.toml")?;

    let mut config: Config = toml::from_str(&contents).context("Could not parse ena.toml")?;

    config.scraping.boards.sort();
    config.scraping.boards.dedup();

    if config.scraping.poll_interval == 0 {
        return Err(ConfigError::ZeroPollInterval.into());
    } else if config.scraping.poll_interval < 10 {
        warn!("4chan API rules recommend a minimum `poll_interval` of 10 seconds");
        warn!("A very short `poll_interval` may cause the API to return old data");
    }

    Ok(config)
}

use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;
use std::path::PathBuf;

use failure::{Error, ResultExt};

use four_chan::Board;

#[derive(Deserialize)]
pub struct Config {
    pub boards: Vec<Board>,
    pub poll_interval: u64,
    pub fetch_archive: bool,
    pub download_media: bool,
    pub download_thumbs: bool,
    pub media_rate_limiting: RateLimitingConfig,
    pub thread_rate_limiting: RateLimitingConfig,
    pub thread_list_rate_limiting: RateLimitingConfig,
    pub database_url: String,
    pub charset: String,
    pub media_path: PathBuf,
    pub adjust_timestamps: bool,
    pub refetch_archived_threads: bool,
    pub always_add_archive_times: bool,
    pub create_index_counters: bool,
}

#[derive(Deserialize)]
pub struct RateLimitingConfig {
    pub interval: u64,
    pub max_interval: usize,
    pub max_concurrent: usize,
}

#[derive(Debug, Fail)]
pub enum ConfigError {
    #[fail(display = "`poll_interval` must be at least 1 second (preferably 10 seconds or more)")]
    ZeroPollInterval,
}

pub fn parse_config() -> Result<Config, Error> {
    let file = File::open("ena.toml").context("Could not open ena.toml")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader
        .read_to_string(&mut contents)
        .context("Could not read ena.toml")?;

    let mut config: Config = toml::from_str(&contents).context("Could not parse ena.toml")?;

    config.boards.sort();
    config.boards.dedup();

    if config.poll_interval == 0 {
        return Err(ConfigError::ZeroPollInterval.into());
    } else if config.poll_interval < 10 {
        warn!("4chan API rules recommend a minimum `poll_interval` of 10 seconds");
        warn!("A very short `poll_interval` may cause the API to return old data");
    }

    Ok(config)
}

//! Configuration file parsing.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{prelude::*, BufReader},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use failure::{Fail, ResultExt};
use serde::{de::Error, Deserialize, Deserializer};
use toml::Value;

use crate::four_chan::Board;

#[derive(Deserialize)]
pub struct Config {
    #[serde(skip_deserializing)]
    pub boards: Arc<HashMap<Board, ScrapingConfig>>,
    pub network: NetworkConfig,
    pub database_media: DatabaseMediaConfig,
    pub asagi_compat: AsagiCompatibilityConfig,
}

#[derive(Deserialize)]
pub struct ScrapingConfig {
    #[serde(deserialize_with = "nonzero_duration_from_secs")]
    pub poll_interval: Duration,
    pub fetch_archive: bool,
    pub download_media: bool,
    pub download_thumbs: bool,
}

impl ScrapingConfig {
    fn merge(&self, board: &OptionScrapingConfig) -> Self {
        Self {
            poll_interval: board.poll_interval.unwrap_or(self.poll_interval),
            fetch_archive: board.fetch_archive.unwrap_or(self.fetch_archive),
            download_media: board.download_media.unwrap_or(self.download_media),
            download_thumbs: board.download_thumbs.unwrap_or(self.download_thumbs),
        }
    }
}

/// Used to extract the global and board scraping configs for merging and insertion into Config.
#[derive(Deserialize)]
struct BoardsConfig {
    scraping: ScrapingConfig,
    boards: HashMap<String, OptionScrapingConfig>,
}

#[derive(Deserialize)]
pub struct OptionScrapingConfig {
    #[serde(default)]
    #[serde(deserialize_with = "option_nonzero_duration_from_secs")]
    pub poll_interval: Option<Duration>,
    pub fetch_archive: Option<bool>,
    pub download_media: Option<bool>,
    pub download_thumbs: Option<bool>,
}

#[derive(Deserialize)]
pub struct NetworkConfig {
    pub rate_limiting: RateLimitingConfig,
    pub retry_backoff: RetryBackoffConfig,
}

#[derive(Deserialize)]
pub struct RateLimitingConfig {
    pub media: RateLimitingSettings,
    pub thread: RateLimitingSettings,
    pub thread_list: RateLimitingSettings,
}

#[derive(Deserialize)]
pub struct RateLimitingSettings {
    #[serde(deserialize_with = "nonzero_duration_from_secs")]
    pub interval: Duration,
    #[serde(deserialize_with = "validate_max_interval")]
    pub max_interval: usize,
    #[serde(deserialize_with = "validate_max_concurrent")]
    pub max_concurrent: usize,
}

#[derive(Clone, Copy, Deserialize)]
pub struct RetryBackoffConfig {
    #[serde(deserialize_with = "nonzero_duration_from_secs")]
    pub base: Duration,
    pub factor: u32,
    #[serde(deserialize_with = "duration_from_secs")]
    pub max: Duration,
}

#[derive(Deserialize)]
pub struct DatabaseMediaConfig {
    #[serde(deserialize_with = "nonempty_string")]
    pub database_url: String,
    #[serde(deserialize_with = "nonempty_string")]
    pub charset: String,
    #[serde(deserialize_with = "pathbuf_from_string")]
    pub media_path: PathBuf,
}

#[derive(Deserialize)]
pub struct AsagiCompatibilityConfig {
    pub adjust_timestamps: bool,
    pub refetch_archived_threads: bool,
    pub always_add_archive_times: bool,
    pub create_index_counters: bool,
}

/// Configuration parsing errors.
///
/// Note: most of the configuration checking is done through (a kludge of) Serde's
/// `deserialize_with` attributes and the `deserialize_validate!` macro. This enum is used for
/// errors which don't work well with Serde's custom error message format.
#[derive(Debug, Fail)]
pub enum ConfigError {
    #[fail(display = "Invalid config: `boards` must contain at least one board")]
    NoBoards,

    #[fail(display = "Invalid config: `network.retry_backoff.factor` must be at least 2")]
    SmallRetryFactor,
}

/// Read the configuration file `ena.toml` and parse it.
pub fn parse_config() -> Result<Config, failure::Error> {
    let file = File::open("ena.toml").context("Could not open ena.toml")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader
        .read_to_string(&mut contents)
        .context("Could not read ena.toml")?;

    let boards_config: BoardsConfig =
        toml::from_str(&contents).context("Could not parse ena.toml")?;
    let mut config: Config = toml::from_str(&contents).context("Could not parse ena.toml")?;

    if boards_config.boards.is_empty() {
        return Err(ConfigError::NoBoards.into());
    } else if config.network.retry_backoff.factor < 2 {
        return Err(ConfigError::SmallRetryFactor.into());
    }

    fs::create_dir_all(&config.database_media.media_path)
        .context("Could not create media directory")?;
    let mut test_file = config.database_media.media_path.clone();
    test_file.push("ena_permission_test");
    File::create(&test_file).context("Could not create test file in media directory")?;
    fs::remove_file(&test_file).context("Could not remove media directory permission test file")?;

    let boards = Arc::get_mut(&mut config.boards).unwrap();
    for (board, mut config) in boards_config.boards.into_iter() {
        let board: Board =
            Value::try_into(Value::String(board)).context("Could not parse `boards`")?;
        if !board.is_archived() && config.fetch_archive.unwrap_or(false) {
            warn!(
                "/{}/ is not an archived board, ignoring `fetch_archive = true`",
                board
            );
            config.fetch_archive = Some(false);
        }
        boards.insert(board, boards_config.scraping.merge(&config));
    }
    boards.shrink_to_fit();

    if config
        .boards
        .values()
        .any(|config| config.poll_interval.as_secs() < 10)
    {
        warn!("4chan API rules recommend a minimum `poll_interval` of 10 seconds");
        warn!("A very short `poll_interval` may cause the API to return old data");
    }

    Ok(config)
}

/// Create a function for use with Serde's `deserialize_with` attribute which deserializes and/or
/// validates a field.
// This is a kludge, but it allow us to print error messages with context and doesn't require
// procedural macros, another dependency, or copy-pasted code.
macro_rules! deserialize_validate {
    ($name:ident, $type:ty, $pred:expr, $err:expr $(,)*) => {
        deserialize_validate!(
            $name,
            $type => $type,
            $pred,
            |a| a,
            $err,
        );
    };

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
    nonempty_string,
    String,
    |s: &str| !s.is_empty(),
    "string must not be empty",
);

deserialize_validate!(
    pathbuf_from_string,
    String => PathBuf,
    |s: &str| !s.is_empty(),
    |s| PathBuf::from(s),
    "path must not be empty (use \".\" for current dir)",
);

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
    option_nonzero_duration_from_secs,
    Option<u64> => Option<Duration>,
    |secs: &Option<u64>| secs.map_or(true, |s| s != 0),
    |secs: Option<u64>| secs.map(Duration::from_secs),
    "interval must be at least 1 second",
);

deserialize_validate!(
    validate_max_interval,
    usize,
    |&max| max != 0,
    "`max_interval` must be at least 1",
);

deserialize_validate!(
    validate_max_concurrent,
    usize,
    |&max| max != 0,
    "`max_concurrent` must be at least 1",
);

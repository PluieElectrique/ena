extern crate actix;
extern crate chrono;
#[macro_use]
extern crate failure;
extern crate futures;
extern crate hyper;
extern crate hyper_tls;
#[macro_use]
extern crate log;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate toml;

macro_rules! impl_enum_from {
    ($ext_type:ty, $enum:ident, $variant:ident) => {
        impl From<$ext_type> for $enum {
            fn from(err: $ext_type) -> Self {
                $enum::$variant(err)
            }
        }
    };
}

#[macro_export]
macro_rules! log_error {
    ($fail:expr) => {{
        let fail: &::failure::Fail = $fail;
        let mut pretty = fail.to_string();
        for cause in fail.iter_causes() {
            pretty.push_str(": ");
            pretty.push_str(&cause.to_string());
        }
        error!("{}", pretty);
    }};
}
#[macro_export]
macro_rules! log_warn {
    ($fail:expr) => {{
        let fail: &::failure::Fail = $fail;
        let mut pretty = fail.to_string();
        for cause in fail.iter_causes() {
            pretty.push_str(": ");
            pretty.push_str(&cause.to_string());
        }
        warn!("{}", pretty);
    }};
}

pub mod actors;
pub mod four_chan;

use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;

use failure::{Error, ResultExt};

pub const BOARD_REPLACE: &str = "%%BOARD%%";
pub const CHARSET_REPLACE: &str = "%%CHARSET%%";
pub const BOARD_SQL: &str = include_str!("sql/boards.sql");
pub const COMMON_SQL: &str = include_str!("sql/common.sql");

#[derive(Deserialize)]
pub struct Config {
    pub boards: Vec<four_chan::Board>,
    pub charset: String,
    pub database_url: String,
    pub poll_interval: u64,
}

#[derive(Debug, Fail)]
pub enum ConfigError {
    #[fail(display = "`poll_interval` must be at least 1 second (preferably 10 seconds or more)")]
    ZeroPollInterval,
}

pub fn parse_config() -> Result<Config, Error> {
    let file = File::open("config.toml").context("Could not open config.toml")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader
        .read_to_string(&mut contents)
        .context("Could not read config.toml")?;

    let toml: Config = toml::from_str(&contents).context("Could not parse config.toml")?;

    if toml.poll_interval == 0 {
        return Err(ConfigError::ZeroPollInterval.into());
    } else if toml.poll_interval < 10 {
        warn!("4chan API rules recommend a minimum `poll_interval` of 10 seconds");
        warn!("A very short `poll_interval` may cause the API to return old data");
    } else if toml.poll_interval > 1800 {
        warn!("A `poll_interval` of more than 30min may result in lost data");
    }
    Ok(toml)
}

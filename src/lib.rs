extern crate actix;
extern crate chrono;
extern crate chrono_tz;
#[macro_use]
extern crate failure;
extern crate futures;
#[macro_use]
extern crate html5ever;
extern crate hyper;
extern crate hyper_tls;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
#[macro_use]
extern crate mysql_async as my;
extern crate regex;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate tokio;
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
pub mod html;

use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;

use failure::{Error, ResultExt};

#[derive(Deserialize)]
pub struct Config {
    pub boards: Vec<four_chan::Board>,
    pub poll_interval: u64,
    pub deleted_page_threshold: u8,
    pub database_url: String,
    pub charset: String,
    pub adjust_timestamps: bool,
}

#[derive(Debug, Fail)]
pub enum ConfigError {
    #[fail(display = "`poll_interval` must be at least 1 second (preferably 10 seconds or more)")]
    ZeroPollInterval,
    #[fail(display = "`deleted_page_threshold` must be a number from 0-10")]
    OutOfRangeThreshold,
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
    if toml.deleted_page_threshold > 10 {
        return Err(ConfigError::OutOfRangeThreshold.into());
    }

    Ok(toml)
}

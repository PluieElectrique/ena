extern crate actix;
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

pub mod actors;
pub mod four_chan;

use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;

use failure::{Error, ResultExt};

pub const BOARD_SQL: &str = include_str!("sql/boards.sql");
pub const COMMON_SQL: &str = include_str!("sql/common.sql");

#[derive(Deserialize)]
pub struct Config {
    pub boards: Vec<four_chan::Board>,
    pub charset: String,
    pub database_url: String,
    pub poll_interval: u64,
}

pub fn parse_config() -> Result<Config, Error> {
    let file = File::open("config.toml").context("Could not open config.toml")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader
        .read_to_string(&mut contents)
        .context("Could not read config.toml")?;

    Ok(toml::from_str(&contents).context("Could not parse config.toml")?)
}

pub fn print_error(err: &Error) {
    let mut pretty = err.to_string();
    for cause in err.iter_causes() {
        pretty.push_str(": ");
        pretty.push_str(&cause.to_string());
    }
    error!("{}", pretty);
}

extern crate hyper;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate toml;

pub mod four_chan;

use std::fs::File;
use std::io::prelude::*;
use std::io::{self, BufReader};

pub const BOARD_SQL: &str = include_str!("sql/boards.sql");
pub const COMMON_SQL: &str = include_str!("sql/common.sql");

#[derive(Deserialize)]
pub struct Config {
    pub boards: Vec<String>,
    pub charset: String,
    pub database_url: String,
}

pub fn parse_config() -> io::Result<Config> {
    let file = File::open("config.toml")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader.read_to_string(&mut contents)?;

    // TODO: don't unwrap this and use failure or something
    Ok(toml::from_str(&contents).unwrap())
}

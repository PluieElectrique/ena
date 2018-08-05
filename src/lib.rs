extern crate actix;
extern crate futures;
extern crate hyper;
extern crate hyper_tls;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate toml;

pub mod four_chan;

pub use hyper::client::connect::Connect;

use std::fs::File;
use std::io::prelude::*;
use std::io::{self, BufReader};

pub const BOARD_SQL: &str = include_str!("sql/boards.sql");
pub const COMMON_SQL: &str = include_str!("sql/common.sql");

#[derive(Deserialize)]
pub struct Config {
    pub boards: Vec<four_chan::Board>,
    pub charset: String,
    pub database_url: String,
}

pub fn parse_config() -> io::Result<Config> {
    let file = File::open("config.toml")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader.read_to_string(&mut contents)?;

    // TODO: don't unwrap this and use failure or something
    Ok(toml::from_str(&contents).expect("Could not parse config file"))
}

//! A 4chan scraper. Currently designed to be a (mostly) compatible, improved replacement for Asagi.

extern crate actix;
extern crate chrono;
extern crate chrono_tz;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate futures;
extern crate hyper;
extern crate hyper_tls;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
#[macro_use]
extern crate mysql_async as my;
extern crate pest;
#[macro_use]
extern crate pest_derive;
extern crate regex;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate tokio;
extern crate toml;
extern crate twox_hash;

macro_rules! impl_enum_from {
    ($ext_type:ty, $enum:ident, $variant:ident) => {
        impl From<$ext_type> for $enum {
            fn from(err: $ext_type) -> Self {
                $enum::$variant(err)
            }
        }
    };
}

macro_rules! zero_format {
    ($str:expr, $expr:expr) => {{
        let num = $expr;
        if num == 0 {
            String::new()
        } else {
            format!($str, num)
        }
    }};
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

pub mod actors;
pub mod config;
pub mod four_chan;
pub mod html;

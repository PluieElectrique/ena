//! A 4chan scraper. Currently designed to be a (mostly) compatible, improved replacement for Asagi.

// The logging macros are basic enough that we shouldn't have to import them whenever we want to use
// them. So, we make them globally available.
#[macro_use]
extern crate log;

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

/// A helper macro for logging an error and its causes.
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

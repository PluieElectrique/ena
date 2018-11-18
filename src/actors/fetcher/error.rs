use std;

use actix;
use hyper;
use serde_json;
use tokio;

#[derive(Debug, Fail)]
pub enum FetchError {
    #[fail(display = "Hyper error: {}", _0)]
    HyperError(hyper::Error),

    #[fail(display = "Bad status: {}", _0)]
    BadStatus(hyper::StatusCode),

    #[fail(display = "JSON error: {}", _0)]
    JsonError(serde_json::Error),

    #[fail(display = "Resource not modified")]
    NotModified,

    #[fail(display = "Media already exists")]
    ExistingMedia,

    #[fail(display = "Resource not found: {}", _0)]
    NotFound(String),

    #[fail(display = "API returned empty data")]
    EmptyData,

    #[fail(display = "Timer error: {}", _0)]
    TimerError(tokio::timer::Error),

    #[fail(display = "IO error: {}", _0)]
    IoError(std::io::Error),

    #[fail(display = "Mailbox error: {}", _0)]
    MailboxError(actix::MailboxError),
}

impl_enum_from!(hyper::Error, FetchError, HyperError);
impl_enum_from!(hyper::StatusCode, FetchError, BadStatus);
impl_enum_from!(serde_json::Error, FetchError, JsonError);
impl_enum_from!(tokio::timer::Error, FetchError, TimerError);
impl_enum_from!(std::io::Error, FetchError, IoError);
impl_enum_from!(actix::MailboxError, FetchError, MailboxError);

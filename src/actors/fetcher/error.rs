use std;

use actix;
use hyper;
use serde_json;
use tokio;

#[derive(Debug, Fail)]
pub enum FetchError {
    #[fail(display = "Bad status: {}", _0)]
    BadStatus(hyper::StatusCode),

    #[fail(display = "API returned empty data")]
    EmptyData,

    #[fail(display = "Media already exists")]
    ExistingMedia,

    #[fail(display = "Hyper error: {}", _0)]
    HyperError(hyper::Error),

    #[fail(display = "Thread has invalid `resto` values")]
    InvalidReplyTo,

    #[fail(display = "IO error: {}", _0)]
    IoError(std::io::Error),

    #[fail(display = "JSON error: {}", _0)]
    JsonError(serde_json::Error),

    #[fail(display = "Mailbox error: {}", _0)]
    MailboxError(actix::MailboxError),

    #[fail(display = "Resource not found: {}", _0)]
    NotFound(String),

    #[fail(display = "Resource not modified")]
    NotModified,

    #[fail(display = "Timer error: {}", _0)]
    TimerError(tokio::timer::Error),
}

macro_rules! impl_enum_from {
    ($ext_type:ty, $enum:ident, $variant:ident) => {
        impl From<$ext_type> for $enum {
            fn from(err: $ext_type) -> Self {
                $enum::$variant(err)
            }
        }
    };
}

impl_enum_from!(hyper::Error, FetchError, HyperError);
impl_enum_from!(hyper::StatusCode, FetchError, BadStatus);
impl_enum_from!(serde_json::Error, FetchError, JsonError);
impl_enum_from!(tokio::timer::Error, FetchError, TimerError);
impl_enum_from!(std::io::Error, FetchError, IoError);
impl_enum_from!(actix::MailboxError, FetchError, MailboxError);

use failure::Fail;

#[derive(Debug, Fail)]
pub enum FetchError {
    #[fail(display = "Bad status: {}", _0)]
    BadStatus(hyper::StatusCode),

    #[fail(display = "Thread has no posts")]
    EmptyThread,

    #[fail(display = "Media already exists")]
    ExistingMedia,

    #[fail(display = "Hyper error: {}", _0)]
    HyperError(hyper::Error),

    #[fail(display = "Thread has invalid `resto` values")]
    InvalidReplyTo,

    #[fail(display = "Invalid URI: {}", _0)]
    InvalidUri(hyper::http::uri::InvalidUri),

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
    ($variant:ident, $ext_type:ty) => {
        impl From<$ext_type> for FetchError {
            fn from(err: $ext_type) -> Self {
                FetchError::$variant(err)
            }
        }
    };
}

impl_enum_from!(BadStatus, hyper::StatusCode);
impl_enum_from!(HyperError, hyper::Error);
impl_enum_from!(InvalidUri, hyper::http::uri::InvalidUri);
impl_enum_from!(IoError, std::io::Error);
impl_enum_from!(JsonError, serde_json::Error);
impl_enum_from!(MailboxError, actix::MailboxError);
impl_enum_from!(TimerError, tokio::timer::Error);

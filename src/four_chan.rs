//! 4chan's API. Not implemented: getting the page or catalog of a board or boards.json

use std::collections::HashMap;
use std::default::Default;
use std::fmt;

use actix::prelude::*;
use chrono::prelude::*;
use futures::future;
use futures::prelude::*;
use hyper::client::{Client, HttpConnector};
use hyper::header::{self, HeaderValue};
use hyper::{self, Body, StatusCode};
use hyper_tls::HttpsConnector;
use serde::{Deserialize, Deserializer};
use serde_json;

const API_PREFIX: &str = "https://a.4cdn.org/";
const RFC_1123_FORMAT: &str = "%a, %d %b %Y %T GMT";

pub struct Fetcher {
    client: Client<HttpsConnector<HttpConnector>>,
    last_fetched: HashMap<FetchKey, DateTime<Utc>>,
}

impl Fetcher {
    fn fetch_with_last_modified<K: Into<FetchKey>>(
        &mut self,
        uri: hyper::Uri,
        key: K,
    ) -> impl Future<Item = hyper::Chunk, Error = FetchError> {
        let mut request = hyper::Request::get(uri).body(Body::default()).unwrap();

        let last_fetched = self.last_fetched.insert(key.into(), Utc::now());
        if let Some(last_fetched) = last_fetched {
            let last_fetched = HeaderValue::from_str(
                last_fetched.format(RFC_1123_FORMAT).to_string().as_str(),
            ).unwrap();
            request
                .headers_mut()
                .insert(header::IF_MODIFIED_SINCE, last_fetched);
        }
        self.client
            .request(request)
            .from_err()
            .and_then(move |res| {
                if let (Some(last_fetched), Some(last_modified)) =
                    (last_fetched, res.headers().get(header::LAST_MODIFIED))
                {
                    let last_modified = Utc
                        .datetime_from_str(last_modified.to_str().unwrap(), RFC_1123_FORMAT)
                        .unwrap();

                    if last_fetched > last_modified {
                        let now = Utc::now();
                        warn!(
                            "API returned old data: last fetched {} ago, last modified {} ago",
                            now - last_fetched,
                            now - last_modified,
                        );
                        return future::err(FetchError::NotModified);
                    }
                }

                if StatusCode::OK == res.status() {
                    future::ok(res)
                } else if StatusCode::NOT_MODIFIED == res.status() {
                    future::err(FetchError::NotModified)
                } else {
                    future::err(FetchError::BadStatus(res.status().to_string()))
                }
            })
            .and_then(|res| res.into_body().concat2().from_err())
    }
}

// We don't include a Thread variant because we don't need to look at the Last-Modified header.
// threads.json already gives us last_modified.
#[derive(Eq, Hash, PartialEq)]
enum FetchKey {
    Threads(FetchThreads),
    Archive(FetchArchive),
}
impl_enum_from!(FetchThreads, FetchKey, Threads);
impl_enum_from!(FetchArchive, FetchKey, Archive);

impl Default for Fetcher {
    fn default() -> Self {
        let https = HttpsConnector::new(2).expect("Could not create HttpsConnector");
        let client = Client::builder().build::<_, Body>(https);
        Self {
            client,
            last_fetched: HashMap::new(),
        }
    }
}

impl Actor for Fetcher {
    type Context = Context<Self>;
}

impl Supervised for Fetcher {}
impl SystemService for Fetcher {}

fn get_uri(path: &str) -> hyper::Uri {
    let mut uri = String::from(API_PREFIX);
    uri.push_str(&path);
    uri.parse().unwrap_or_else(|err| {
        panic!("Could not parse URI {}: {}", uri, err);
    })
}

// TODO: Use the Error/ErrorKind pattern if we need context
#[derive(Debug, Fail)]
pub enum FetchError {
    #[fail(display = "Hyper error: {}", _0)]
    HyperError(#[cause] hyper::Error),

    #[fail(display = "Bad status: {}", _0)]
    BadStatus(String),

    #[fail(display = "JSON error: {}", _0)]
    JsonError(#[cause] serde_json::Error),

    #[fail(display = "Resource not modified")]
    NotModified,
}
impl_enum_from!(hyper::Error, FetchError, HyperError);
impl_enum_from!(serde_json::Error, FetchError, JsonError);

#[derive(Debug)]
pub struct FetchThread(pub Board, pub u64);
impl Message for FetchThread {
    type Result = Result<Vec<Post>, FetchError>;
}

impl Handler<FetchThread> for Fetcher {
    type Result = ResponseFuture<Vec<Post>, FetchError>;

    fn handle(&mut self, msg: FetchThread, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.client
                .get(get_uri(&format!("{}/thread/{}.json", msg.0, msg.1)))
                .and_then(|res| res.into_body().concat2())
                .from_err()
                .and_then(move |body| {
                    let PostsWrapper { posts } = serde_json::from_slice(&body)?;
                    Ok(posts)
                }),
        )
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct FetchThreads(pub Board);
impl Message for FetchThreads {
    type Result = Result<Vec<Thread>, FetchError>;
}

impl Handler<FetchThreads> for Fetcher {
    type Result = ResponseFuture<Vec<Thread>, FetchError>;
    fn handle(&mut self, msg: FetchThreads, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.fetch_with_last_modified(get_uri(&format!("{}/threads.json", msg.0)), msg)
                .and_then(move |body| {
                    let threads: Vec<ThreadPage> = serde_json::from_slice(&body)?;
                    Ok(threads.into_iter().fold(vec![], |mut acc, mut t| {
                        acc.append(&mut t.threads);
                        acc
                    }))
                }),
        )
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct FetchArchive(pub Board);
impl Message for FetchArchive {
    type Result = Result<Vec<u64>, FetchError>;
}

impl Handler<FetchArchive> for Fetcher {
    type Result = ResponseFuture<Vec<u64>, FetchError>;
    fn handle(&mut self, msg: FetchArchive, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.fetch_with_last_modified(get_uri(&format!("{}/archive.json", msg.0)), msg)
                .and_then(move |body| Ok(serde_json::from_slice(&body)?)),
        )
    }
}

#[derive(Deserialize)]
struct ThreadPage {
    threads: Vec<Thread>,
}

#[derive(Debug, Deserialize)]
pub struct Thread {
    pub no: u64,
    pub last_modified: u64,
}

#[derive(Deserialize)]
struct PostsWrapper {
    posts: Vec<Post>,
}

/// Some fields aren't used, and thus are omitted.
#[derive(Debug, Deserialize)]
pub struct Post {
    // Required fields
    no: u64,
    #[serde(rename = "resto")]
    reply_to: u64,
    time: u64,

    // Optional fields
    /// Only blank when name is blank and trip is provided
    name: Option<String>,
    trip: Option<String>,
    /// Displays if board has DISPLAY_ID set
    id: Option<String>,
    #[serde(default = "capcode_default")]
    capcode: String,
    country: Option<String>,
    #[serde(rename = "sub")]
    subject: Option<String>,
    #[serde(rename = "com")]
    comment: Option<String>,
    #[serde(rename = "tim")]
    time_millis: Option<u64>,

    // OP-only fields
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    sticky: bool,
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    closed: bool,
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    archived: bool,
    archived_on: Option<u64>,

    #[serde(flatten)]
    image: Option<PostImage>,
}

#[derive(Debug, Deserialize)]
pub struct PostImage {
    filename: String,
    ext: String,
    #[serde(rename = "fsize")]
    filesize: u32,
    md5: String,
    #[serde(rename = "w")]
    image_width: u16,
    #[serde(rename = "h")]
    image_height: u16,
    #[serde(rename = "tn_w")]
    thumbnail_width: u8,
    #[serde(rename = "tn_h")]
    thumbnail_height: u8,
    #[serde(rename = "filedeleted")]
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    file_deleted: bool,
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    spoiler: bool,
}

fn capcode_default() -> String {
    String::from("N")
}

fn num_to_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let n: u8 = Deserialize::deserialize(deserializer)?;
    if n == 1 {
        Ok(true)
    } else if n == 0 {
        Ok(false)
    } else {
        use serde::de::Error;
        Err(D::Error::custom("Numeric boolean was not 0 or 1"))
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Board::_3 = self {
            write!(f, "3")
        } else {
            fmt::Debug::fmt(self, f)
        }
    }
}

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
pub enum Board {
    _3,
    a,
    aco,
    adv,
    an,
    asp,
    b,
    bant,
    biz,
    c,
    cgl,
    ck,
    cm,
    co,
    d,
    diy,
    e,
    f,
    fa,
    fit,
    g,
    gd,
    gif,
    h,
    hc,
    his,
    hm,
    hr,
    i,
    ic,
    int,
    jp,
    k,
    lgbt,
    lit,
    m,
    mlp,
    mu,
    n,
    news,
    o,
    out,
    p,
    po,
    pol,
    qa,
    qst,
    r,
    r9k,
    s,
    s4s,
    sci,
    soc,
    sp,
    t,
    tg,
    toy,
    trash,
    trv,
    tv,
    u,
    v,
    vg,
    vip,
    vp,
    vr,
    w,
    wg,
    wsg,
    wsr,
    x,
    y,
}

//! 4chan's API. Not implemented: getting the page or catalog of a board or boards.json

use std::default::Default;
use std::fmt;

use actix::prelude::*;
use failure::{Error, ResultExt};
use futures::prelude::*;
use hyper;
use hyper::client::{Client, HttpConnector};
use hyper_tls::HttpsConnector;
use serde::{Deserialize, Deserializer};
use serde_json;

const API_PREFIX: &str = "https://a.4cdn.org/";

pub struct Fetcher {
    client: Client<HttpsConnector<HttpConnector>>,
}

impl Default for Fetcher {
    fn default() -> Self {
        let https = HttpsConnector::new(2).expect("Could not create HttpsConnector");
        let client = Client::builder().build::<_, hyper::Body>(https);
        Self { client }
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

// TODO: Using Error for Fetcher might be too expensive. If hyper errors are common enough, we
// should use #[derive(Fail)] or Error/ErrorKind instead

#[derive(Debug)]
pub struct FetchThread(pub Board, pub u64);
impl Message for FetchThread {
    type Result = Result<Vec<Post>, Error>;
}

impl Handler<FetchThread> for Fetcher {
    type Result = ResponseFuture<Vec<Post>, Error>;

    fn handle(&mut self, msg: FetchThread, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.client
                .get(get_uri(&format!("{}/thread/{}.json", msg.0, msg.1)))
                .and_then(|res| res.into_body().concat2())
                .map_err(|e| e.into())
                .and_then(move |body| {
                    let PostsWrapper { posts } = serde_json::from_slice(&body)
                        .context(format!("Failed to deserialize a post: {:?}", msg))?;
                    Ok(posts)
                }),
        )
    }
}

pub struct FetchThreads(pub Board);
impl Message for FetchThreads {
    type Result = Result<Vec<Thread>, Error>;
}

impl Handler<FetchThreads> for Fetcher {
    type Result = ResponseFuture<Vec<Thread>, Error>;
    fn handle(&mut self, msg: FetchThreads, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.client
                .get(get_uri(&format!("{}/threads.json", msg.0)))
                .and_then(|res| res.into_body().concat2())
                .map_err(|e| e.into())
                .and_then(move |body| {
                    let threads: Vec<ThreadPage> = serde_json::from_slice(&body)
                        .context(format!("Failed to deserialize threads.json from {}", msg.0))?;
                    Ok(threads.into_iter().fold(vec![], |mut acc, mut t| {
                        acc.append(&mut t.threads);
                        acc
                    }))
                }),
        )
    }
}

pub struct FetchArchive(pub Board);
impl Message for FetchArchive {
    type Result = Result<Vec<u64>, Error>;
}

impl Handler<FetchArchive> for Fetcher {
    type Result = ResponseFuture<Vec<u64>, Error>;
    fn handle(&mut self, msg: FetchArchive, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.client
                .get(get_uri(&format!("{}/archive.json", msg.0)))
                .and_then(|res| res.into_body().concat2())
                .map_err(|e| e.into())
                .and_then(move |body| {
                    Ok(serde_json::from_slice(&body)
                        .context(format!("Failed to deserialize the archive of {}", msg.0))?)
                }),
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
#[derive(Clone, Copy, Debug, Deserialize)]
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
    hm,
    hr,
    i,
    ic,
    his,
    int,
    jp,
    k,
    lit,
    lgbt,
    m,
    mlp,
    mu,
    news,
    n,
    o,
    out,
    p,
    po,
    pol,
    qst,
    r,
    r9k,
    s4s,
    s,
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

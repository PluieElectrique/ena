//! 4chan's API. Not implemented: getting the page or catalog of a board or boards.json

use std::default::Default;
use std::fmt;

use actix::prelude::*;
use futures::prelude::*;
use hyper;
use hyper::client::{Client, HttpConnector};
use hyper_tls::HttpsConnector;
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

fn get_uri(path: String) -> hyper::Uri {
    let mut uri = String::from(API_PREFIX);
    uri.push_str(&path);
    uri.parse().expect("Could not parse URI")
}

pub struct FetchThread(pub Board, pub u64);
impl Message for FetchThread {
    type Result = Result<(), hyper::Error>;
}

impl Handler<FetchThread> for Fetcher {
    type Result = ResponseFuture<(), hyper::Error>;

    fn handle(&mut self, msg: FetchThread, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.client
                .get(get_uri(format!("{}/thread/{}.json", msg.0, msg.1)))
                .and_then(|res| res.into_body().concat2())
                .and_then(|_body| {
                    //unimplemented!()
                    Ok(())
                }),
        )
    }
}

pub struct FetchThreads(pub Board);
impl Message for FetchThreads {
    type Result = Result<Vec<Thread>, hyper::Error>;
}

impl Handler<FetchThreads> for Fetcher {
    type Result = ResponseFuture<Vec<Thread>, hyper::Error>;
    fn handle(&mut self, msg: FetchThreads, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.client
                .get(get_uri(format!("{}/threads.json", msg.0)))
                .and_then(|res| res.into_body().concat2())
                .and_then(|body| {
                    let threads: Vec<ThreadPage> =
                        serde_json::from_slice(&body).expect("Deserializing a thread failed");
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
    type Result = Result<Vec<u64>, hyper::Error>;
}

impl Handler<FetchArchive> for Fetcher {
    type Result = ResponseFuture<Vec<u64>, hyper::Error>;
    fn handle(&mut self, msg: FetchArchive, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.client
                .get(get_uri(format!("{}/archive.json", msg.0)))
                .and_then(|res| res.into_body().concat2())
                .and_then(|body| {
                    Ok(serde_json::from_slice(&body).expect("Deserializing the archive failed"))
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Post {
    // Required fields
    no: u64,
    #[serde(rename = "resto")]
    reply_to: u64,
    now: String,
    time: u64,
    #[serde(rename = "tim")]
    time_millis: u64,

    // Optional fields
    // Is only blank when name is blank and trip is provided
    name: String,
    trip: String,
    // Only displays if board has DISPLAY_ID set
    id: String,
    capcode: String,
    country: String,
    country_name: String,
    #[serde(rename = "sub")]
    subject: String,
    #[serde(rename = "com")]
    comment: String,
    since4pass: u16,

    // OP-only fields
    sticky: u8,
    closed: u8,
    archived: u8,
    archived_on: u64,
    replies: u32,
    images: u32,
    #[serde(rename = "bumplimit")]
    bump_limit: u8,
    #[serde(rename = "imagelimit")]
    image_limit: u8,
    semantic_url: String,
    // OP-only index fields
    omitted_posts: u16,
    omitted_images: u16,

    // Image fields
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
    file_deleted: u8,

    // Spoiler fields
    spoiler: u8,
    custom_spoiler: u8,

    // Board-specific fields
    // /f/
    tag: String,
    // /q/
    // TODO: figure this out, Vec<CapcodeReply>?
    #[serde(skip_deserializing)]
    capcode_replies: (),

    // ???
    last_modified: u64,
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

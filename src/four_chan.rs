//! 4chan's API. Not implemented: getting the page or catalog of a board or boards.json

use std::fmt;

use futures::prelude::*;
use hyper::{self, Client};
use serde_json;

use super::Connector;

const API_PREFIX: &str = "https://a.4cdn.org/";

fn get_uri(path: String) -> hyper::Uri {
    let mut uri = String::from(API_PREFIX);
    uri.push_str(&path);
    uri.parse().expect("Could not parse URI")
}

pub fn get_thread<C: Connector>(
    client: &Client<C>,
    board: Board,
    no: u64,
) -> impl Future<Error = hyper::Error> {
    client
        .get(get_uri(format!("{}/thread/{}.json", board, no)))
        .and_then(|res| res.into_body().concat2())
        .and_then(|_body| {
            //unimplemented!()
            Ok(())
        })
}

pub fn get_threads<C: Connector>(
    client: &Client<C>,
    board: Board,
) -> impl Future<Item = Vec<Thread>, Error = hyper::Error> {
    client
        .get(get_uri(format!("{}/threads.json", board)))
        .and_then(|res| res.into_body().concat2())
        .and_then(|body| {
            let threads: Vec<ThreadPage> =
                serde_json::from_slice(&body).expect("Deserializing a thread failed");
            Ok(threads.into_iter().fold(vec![], |mut acc, mut t| {
                acc.append(&mut t.threads);
                acc
            }))
        })
}

pub fn get_archive<C: Connector>(
    client: &Client<C>,
    board: Board,
) -> impl Future<Item = Vec<u64>, Error = hyper::Error> {
    client
        .get(get_uri(format!("{}/archive.json", board)))
        .and_then(|res| res.into_body().concat2())
        .and_then(|body| {
            Ok(serde_json::from_slice(&body).expect("Deserializing the archive failed"))
        })
}

#[derive(Deserialize)]
struct ThreadPage {
    threads: Vec<Thread>,
}

#[derive(Debug, Deserialize)]
pub struct Thread {
    no: u64,
    last_modified: u64,
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

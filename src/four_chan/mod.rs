//! 4chan's API. Not implemented: getting the page or catalog of a board or boards.json

use std::fmt;

use serde::{Deserialize, Deserializer};

mod fetcher;

pub use self::fetcher::*;

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

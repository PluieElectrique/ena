//! 4chan's API. Not implemented: getting the page or catalog of a board or boards.json

use std::fmt;

use serde::{Deserialize, Deserializer};

pub mod fetcher;

#[derive(Deserialize)]
struct ThreadPage {
    page: u8,
    threads: Vec<Thread>,
}

#[derive(Debug, Deserialize)]
pub struct Thread {
    pub no: u64,
    pub last_modified: u64,
    #[serde(skip_deserializing)]
    pub page: u8,
    #[serde(skip_deserializing)]
    pub bump_index: u8,
}

#[derive(Deserialize)]
struct PostsWrapper {
    posts: Vec<Post>,
}

// Some fields aren't used, and thus are omitted.
#[derive(Debug, Deserialize)]
pub struct Post {
    // Required fields
    pub no: u64,
    #[serde(rename = "resto")]
    pub reply_to: u64,
    pub time: u64,

    // Optional fields
    /// Only blank when name is blank and trip is provided
    pub name: Option<String>,
    pub trip: Option<String>,
    /// Displays if board has DISPLAY_ID set
    pub id: Option<String>,
    pub capcode: Option<String>,
    pub country: Option<String>,
    #[serde(rename = "sub")]
    pub subject: Option<String>,
    #[serde(rename = "com")]
    pub comment: Option<String>,

    #[serde(flatten)]
    pub op_data: OpData,

    #[serde(flatten)]
    pub image: Option<PostImage>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct OpData {
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    pub sticky: bool,
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    pub closed: bool,
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    pub archived: bool,
    pub archived_on: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PostImage {
    pub filename: String,
    pub ext: String,
    #[serde(rename = "tim")]
    pub time_millis: u64,
    #[serde(rename = "fsize")]
    pub filesize: u32,
    pub md5: String,
    #[serde(rename = "w")]
    pub image_width: u16,
    #[serde(rename = "h")]
    pub image_height: u16,
    #[serde(rename = "tn_w")]
    pub thumbnail_width: u8,
    #[serde(rename = "tn_h")]
    pub thumbnail_height: u8,
    // The Asagi/FoolFuuka schema currently doesn't track this
    //#[serde(rename = "filedeleted")]
    //#[serde(deserialize_with = "num_to_bool")]
    //#[serde(default)]
    //pub file_deleted: bool,
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    pub spoiler: bool,
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

impl Board {
    pub fn max_threads(self) -> usize {
        match self {
            Board::f => 30,
            _ => 150,
        }
    }

    pub fn is_archived(self) -> bool {
        match self {
            Board::b | Board::bant | Board::f | Board::trash => false,
            _ => true,
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

const FOUR_CHAN_API_PREFIX: &str = "https://a.4cdn.org/";

// TODO: validation of board names and page numbers?
pub enum FourChanApi<'a> {
    Thread(&'a str, u64),
    Threads(&'a str),
    Page(&'a str, u8),
    Catalog(&'a str),
    Archive(&'a str),
    Boards,
}

impl<'a> FourChanApi<'a> {
    pub fn get_uri(&self) -> super::hyper::Uri {
        use self::FourChanApi::*;

        let mut uri = String::from(FOUR_CHAN_API_PREFIX);
        uri.push_str(&match self {
            Thread(board, no) => format!("{}/thread/{}.json", board, no),
            Threads(board) => format!("{}/threads.json", board),
            Page(board, num) => {
                assert!(*num <= 10);
                format!("{}/{}.json", board, num)
            }
            Catalog(board) => format!("{}/catalog.json", board),
            Archive(board) => format!("{}/archive.json", board),
            Boards => String::from("boards.json"),
        });

        uri.parse().expect("Could not parse 4chan API url")
    }
}

#[derive(Deserialize)]
pub struct ThreadPage {
    threads: Vec<Thread>,
}

#[derive(Debug, Deserialize)]
pub struct Thread {
    no: u64,
    last_modified: u64,
}

pub fn deserialize_threads(slice: &[u8]) -> Vec<Thread> {
    let threads: Vec<ThreadPage> =
        super::serde_json::from_slice(slice).expect("JSON deserializing failed");
    threads.into_iter().fold(vec![], |mut acc, mut t| {
        acc.append(&mut t.threads);
        acc
    })
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

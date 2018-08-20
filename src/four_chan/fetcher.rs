use std::cmp;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use actix::prelude::*;
use chrono::prelude::*;
use futures::future;
use futures::prelude::*;
use hyper::client::{Client, HttpConnector};
use hyper::header::{self, HeaderValue};
use hyper::{self, Body, StatusCode, Uri};
use hyper_tls::HttpsConnector;
use serde_json;
use tokio_timer::{self, Delay};

use super::*;

const API_URI_PREFIX: &str = "https://a.4cdn.org";
const IMG_URI_PREFIX: &str = "https://i.4cdn.org";
const RFC_1123_FORMAT: &str = "%a, %d %b %Y %T GMT";

pub struct Fetcher {
    client: Client<HttpsConnector<HttpConnector>>,
    last_modified: HashMap<FetchKey, DateTime<Utc>>,
    last_request: Instant,
    fetch_delay: Duration,
}

impl Actor for Fetcher {
    type Context = Context<Self>;
}

impl Fetcher {
    pub fn new(fetch_delay: Duration) -> Self {
        let https = HttpsConnector::new(2).expect("Could not create HttpsConnector");
        let client = Client::builder().build::<_, Body>(https);
        Self {
            client,
            last_modified: HashMap::new(),
            last_request: Instant::now() - fetch_delay,
            fetch_delay,
        }
    }

    fn get_delay(&mut self) -> Delay {
        self.last_request = cmp::max(Instant::now(), self.last_request + self.fetch_delay);
        Delay::new(self.last_request)
    }

    fn fetch_with_last_modified<K: Into<FetchKey>>(
        &mut self,
        uri: Uri,
        key: K,
        ctx: &Context<Self>,
    ) -> impl Future<Item = hyper::Chunk, Error = FetchError> {
        let mut request = hyper::Request::get(uri).body(Body::default()).unwrap();
        let key = key.into();
        let myself = ctx.address();

        let last_modified = self
            .last_modified
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Utc.timestamp(1_065_062_160, 0));
        request.headers_mut().insert(
            header::IF_MODIFIED_SINCE,
            HeaderValue::from_str(last_modified.format(RFC_1123_FORMAT).to_string().as_str())
                .unwrap(),
        );
        self.client
            .request(request)
            .from_err()
            .and_then(move |res| {
                let new_modified = res
                    .headers()
                    .get(header::LAST_MODIFIED)
                    .map(|new| {
                        Utc.datetime_from_str(new.to_str().unwrap(), RFC_1123_FORMAT)
                            .unwrap()
                    }).unwrap_or_else(Utc::now);

                match res.status() {
                    StatusCode::NOT_MODIFIED => future::err(FetchError::NotModified),
                    StatusCode::OK => {
                        if last_modified > new_modified {
                            warn!(
                                "API sent old data: If-Modified-Since: {}, but Last-Modified: {}",
                                last_modified.format(RFC_1123_FORMAT),
                                new_modified.format(RFC_1123_FORMAT),
                            );
                            return future::err(FetchError::NotModified);
                        }
                        future::ok((res, new_modified))
                    }
                    _ => future::err(res.status().into()),
                }
            }).and_then(move |(res, last_modified)| {
                myself
                    .send(UpdateLastFetched(key, last_modified))
                    .then(|_| res.into_body().concat2().from_err())
            })
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

// TODO: Use the Error/ErrorKind pattern if we need context
#[derive(Debug, Fail)]
pub enum FetchError {
    #[fail(display = "Hyper error: {}", _0)]
    HyperError(#[cause] hyper::Error),

    #[fail(display = "Bad status: {}", _0)]
    BadStatus(hyper::StatusCode),

    #[fail(display = "JSON error: {}", _0)]
    JsonError(#[cause] serde_json::Error),

    #[fail(display = "Resource not modified")]
    NotModified,

    #[fail(display = "Resource not found")]
    NotFound,

    #[fail(display = "API returned empty data")]
    Empty,

    #[fail(display = "API returned empty data")]
    TimerError(#[cause] tokio_timer::Error),
}
impl_enum_from!(hyper::Error, FetchError, HyperError);
impl_enum_from!(hyper::StatusCode, FetchError, BadStatus);
impl_enum_from!(serde_json::Error, FetchError, JsonError);
impl_enum_from!(tokio_timer::Error, FetchError, TimerError);

// We would like to return an ActorFuture from Fetcher, but we can't because ActorFutures can only
// run on their own contexts. So, Fetcher must send a message to itself to update `last_modified`.
#[derive(Message)]
struct UpdateLastFetched(FetchKey, DateTime<Utc>);

impl Handler<UpdateLastFetched> for Fetcher {
    type Result = ();

    fn handle(&mut self, msg: UpdateLastFetched, _ctx: &mut Self::Context) {
        self.last_modified.insert(msg.0, msg.1);
    }
}

#[derive(Debug)]
pub struct FetchThread(pub Board, pub u64);
impl Message for FetchThread {
    type Result = Result<Vec<Post>, FetchError>;
}

impl Into<Uri> for FetchThread {
    fn into(self) -> Uri {
        format!("{}/{}/thread/{}.json", API_URI_PREFIX, self.0, self.1)
            .parse()
            .unwrap_or_else(|err| {
                panic!("Could not parse URI from {:?}: {}", self, err);
            })
    }
}

impl Handler<FetchThread> for Fetcher {
    type Result = ResponseFuture<Vec<Post>, FetchError>;

    fn handle(&mut self, msg: FetchThread, _ctx: &mut Self::Context) -> Self::Result {
        let fetch = self.client.get(msg.into());
        Box::new(
            self.get_delay()
                .from_err()
                .and_then(|_| fetch.from_err())
                .and_then(move |res| match res.status() {
                    StatusCode::OK => future::ok(res),
                    StatusCode::NOT_MODIFIED => {
                        warn!("304 Not Modified on thread fetch. Is last_modified in threads.json being checked?");
                        future::err(FetchError::NotModified)
                    }
                    StatusCode::NOT_FOUND => future::err(FetchError::NotFound),
                    _ => future::err(res.status().into()),
                })
                .and_then(|res| res.into_body().concat2().from_err())
                .and_then(move |body| {
                    let PostsWrapper { posts } = serde_json::from_slice(&body)?;
                    if posts.is_empty() {
                        Err(FetchError::Empty)
                    } else {
                        Ok(posts)
                    }
                }),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FetchThreads(pub Board);
impl Message for FetchThreads {
    type Result = Result<Vec<Thread>, FetchError>;
}

impl Into<Uri> for FetchThreads {
    fn into(self) -> Uri {
        format!("{}/{}/threads.json", API_URI_PREFIX, self.0)
            .parse()
            .unwrap_or_else(|err| {
                panic!("Could not parse URI from {:?}: {}", self, err);
            })
    }
}

impl Handler<FetchThreads> for Fetcher {
    type Result = ResponseFuture<Vec<Thread>, FetchError>;
    fn handle(&mut self, msg: FetchThreads, ctx: &mut Self::Context) -> Self::Result {
        let fetch = self.fetch_with_last_modified(msg.into(), msg, ctx);
        Box::new(
            self.get_delay()
                .from_err()
                .and_then(|_| fetch)
                .and_then(move |body| {
                    let threads: Vec<ThreadPage> = serde_json::from_slice(&body)?;
                    let mut threads = threads.into_iter().fold(vec![], |mut acc, mut page| {
                        for thread in &mut page.threads {
                            thread.page = page.page;
                        }
                        acc.append(&mut page.threads);
                        acc
                    });
                    for (i, thread) in threads.iter_mut().enumerate() {
                        thread.bump_index = i as u8;
                    }
                    if threads.is_empty() {
                        Err(FetchError::Empty)
                    } else {
                        Ok(threads)
                    }
                }),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FetchArchive(pub Board);
impl Message for FetchArchive {
    type Result = Result<Vec<u64>, FetchError>;
}

impl Into<Uri> for FetchArchive {
    fn into(self) -> Uri {
        format!("{}/{}/archive.json", API_URI_PREFIX, self.0)
            .parse()
            .unwrap_or_else(|err| {
                panic!("Could not parse URI from {:?}: {}", self, err);
            })
    }
}

impl Handler<FetchArchive> for Fetcher {
    type Result = ResponseFuture<Vec<u64>, FetchError>;
    fn handle(&mut self, msg: FetchArchive, ctx: &mut Self::Context) -> Self::Result {
        let fetch = self.fetch_with_last_modified(msg.into(), msg, ctx);
        Box::new(
            self.get_delay()
                .from_err()
                .and_then(|_| fetch)
                .and_then(move |body| {
                    let archive: Vec<u64> = serde_json::from_slice(&body)?;
                    if archive.is_empty() {
                        Err(FetchError::Empty)
                    } else {
                        Ok(archive)
                    }
                }),
        )
    }
}

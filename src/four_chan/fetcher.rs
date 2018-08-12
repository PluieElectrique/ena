use std::collections::HashMap;
use std::default::Default;

use actix::prelude::*;
use chrono::prelude::*;
use futures::future;
use futures::prelude::*;
use hyper::client::{Client, HttpConnector};
use hyper::header::{self, HeaderValue};
use hyper::{self, Body, StatusCode};
use hyper_tls::HttpsConnector;
use serde_json;

use super::*;

const API_PREFIX: &str = "https://a.4cdn.org/";
const RFC_1123_FORMAT: &str = "%a, %d %b %Y %T GMT";

pub struct Fetcher {
    client: Client<HttpsConnector<HttpConnector>>,
    last_modified: HashMap<FetchKey, DateTime<Utc>>,
}

impl Fetcher {
    pub fn new() -> Self {
        let https = HttpsConnector::new(2).expect("Could not create HttpsConnector");
        let client = Client::builder().build::<_, Body>(https);
        Self {
            client,
            last_modified: HashMap::new(),
        }
    }

    fn fetch_with_last_modified<K: Into<FetchKey>>(
        &mut self,
        uri: hyper::Uri,
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
                    })
                    .unwrap_or_else(Utc::now);

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
                    _ => future::err(FetchError::BadStatus(res.status())),
                }
            })
            .and_then(move |(res, last_modified)| {
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

impl Actor for Fetcher {
    type Context = Context<Self>;
}

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
    BadStatus(hyper::StatusCode),

    #[fail(display = "JSON error: {}", _0)]
    JsonError(#[cause] serde_json::Error),

    #[fail(display = "Resource not modified")]
    NotModified,

    #[fail(display = "API returned empty data")]
    Empty,
}
impl_enum_from!(hyper::Error, FetchError, HyperError);
impl_enum_from!(serde_json::Error, FetchError, JsonError);

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

impl Handler<FetchThread> for Fetcher {
    type Result = ResponseFuture<Vec<Post>, FetchError>;

    fn handle(&mut self, msg: FetchThread, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.client
                .get(get_uri(&format!("{}/thread/{}.json", msg.0, msg.1)))
                .from_err()
                .and_then(|res| match res.status() {
                    StatusCode::OK => future::ok(res),
                    StatusCode::NOT_MODIFIED => {
                        warn!("304 Not Modified on thread fetch. Is last_modified in threads.json being checked?");
                        future::err(FetchError::NotModified)
                    }
                    _ => future::err(FetchError::BadStatus(res.status())),
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

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct FetchThreads(pub Board);
impl Message for FetchThreads {
    type Result = Result<Vec<Thread>, FetchError>;
}

impl Handler<FetchThreads> for Fetcher {
    type Result = ResponseFuture<Vec<Thread>, FetchError>;
    fn handle(&mut self, msg: FetchThreads, ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.fetch_with_last_modified(get_uri(&format!("{}/threads.json", msg.0)), msg, ctx)
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

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct FetchArchive(pub Board);
impl Message for FetchArchive {
    type Result = Result<Vec<u64>, FetchError>;
}

impl Handler<FetchArchive> for Fetcher {
    type Result = ResponseFuture<Vec<u64>, FetchError>;
    fn handle(&mut self, msg: FetchArchive, ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.fetch_with_last_modified(get_uri(&format!("{}/archive.json", msg.0)), msg, ctx)
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

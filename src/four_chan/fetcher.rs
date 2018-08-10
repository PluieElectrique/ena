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
    fn fetch_with_last_modified<K: Into<FetchKey>>(
        &mut self,
        uri: hyper::Uri,
        key: K,
    ) -> impl Future<Item = hyper::Chunk, Error = FetchError> {
        let mut request = hyper::Request::get(uri).body(Body::default()).unwrap();
        let key = key.into();

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
                    _ => future::err(FetchError::BadStatus(res.status().to_string())),
                }
            })
            .and_then(move |(res, last_modified)| {
                let myself = System::current().registry().get::<Self>();
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

impl Default for Fetcher {
    fn default() -> Self {
        let https = HttpsConnector::new(2).expect("Could not create HttpsConnector");
        let client = Client::builder().build::<_, Body>(https);
        Self {
            client,
            last_modified: HashMap::new(),
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
                .and_then(|res| {
                    if StatusCode::OK == res.status() {
                        future::ok(res)
                    } else if StatusCode::NOT_MODIFIED == res.status() {
                        warn!("304 Not Modified on thread fetch. Is last_modified in threads.json being checked?");
                        future::err(FetchError::NotModified)
                    } else {
                        future::err(FetchError::BadStatus(res.status().to_string()))
                    }
                })
                .and_then(|res| res.into_body().concat2().from_err())
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

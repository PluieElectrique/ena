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
    last_fetched: HashMap<FetchKey, DateTime<Utc>>,
}

impl Fetcher {
    fn fetch_with_last_modified<K: Into<FetchKey>>(
        &mut self,
        uri: hyper::Uri,
        key: K,
    ) -> impl Future<Item = hyper::Chunk, Error = FetchError> {
        let mut request = hyper::Request::get(uri).body(Body::default()).unwrap();
        let key = key.into();

        let last_fetched = self.last_fetched.get(&key).cloned();
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
                        warn!(
                            "API returned old data: If-Modified-Since: {}, but Last-Modified: {}",
                            last_fetched.format(RFC_1123_FORMAT),
                            last_modified.format(RFC_1123_FORMAT),
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
            .and_then(move |res| {
                let myself = System::current().registry().get::<Self>();
                myself
                    .send(UpdateLastFetched(key, Utc::now()))
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

#[derive(Message)]
struct UpdateLastFetched(FetchKey, DateTime<Utc>);

impl Handler<UpdateLastFetched> for Fetcher {
    type Result = ();

    fn handle(&mut self, msg: UpdateLastFetched, _ctx: &mut Self::Context) {
        self.last_fetched.insert(msg.0, msg.1);
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

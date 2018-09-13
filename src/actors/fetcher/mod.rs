use std;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use actix::dev::{MessageResponse, ResponseChannel};
use actix::prelude::*;
use chrono;
use chrono::prelude::*;
use failure::{Error, ResultExt};
use futures::future;
use futures::prelude::*;
use futures::sync::mpsc::{self, Sender};
use hyper::client::{Client, HttpConnector};
use hyper::header::{self, HeaderValue};
use hyper::{self, Body, StatusCode, Uri};
use hyper_tls::HttpsConnector;
use log::Level;
use serde_json;
use tokio;
use tokio::runtime::Runtime;

mod rate_limiter;

use self::rate_limiter::{Consume, RateLimiter};
use four_chan::*;
use RateLimitingConfig;

const RFC_1123_FORMAT: &str = "%a, %d %b %Y %T GMT";

pub struct Fetcher {
    client: Client<HttpsConnector<HttpConnector>>,
    last_modified: HashMap<FetchKey, DateTime<Utc>>,
    media_path: PathBuf,
    media_rl_sender: Sender<Box<Future<Item = (), Error = ()> + Send>>,
    thread_rl_sender: Sender<Box<Future<Item = (), Error = ()>>>,
    thread_list_rl_sender: Sender<Box<Future<Item = (), Error = ()>>>,
    // Fetcher must use its own runtime because tokio::fs functions can't use the current_thread
    // runtime that Actix provides
    runtime: Runtime,
}

impl Actor for Fetcher {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(200);
        // Clean up old Last-Modified values so that we don't leak memory
        ctx.run_interval(Duration::from_secs(86400), |act, _ctx| {
            let yesterday = Utc::now() - chrono::Duration::days(1);
            act.last_modified.retain(|_key, &mut dt| dt > yesterday);
        });
    }
}

impl Fetcher {
    pub fn new(
        media_path: PathBuf,
        media_rl_config: &RateLimitingConfig,
        thread_rl_config: &RateLimitingConfig,
        thread_list_rl_config: &RateLimitingConfig,
    ) -> Result<Self, Error> {
        let mut runtime = Runtime::new().unwrap();
        let https = HttpsConnector::new(2).context("Could not create HttpsConnector")?;
        let client = Client::builder().build::<_, Body>(https);

        let (media_rl_sender, receiver) = mpsc::channel(500);
        runtime.spawn(Consume::new(RateLimiter::new(receiver, media_rl_config)));

        let (thread_rl_sender, receiver) = mpsc::channel(500);
        Arbiter::spawn(Consume::new(RateLimiter::new(receiver, thread_rl_config)));

        let (thread_list_rl_sender, receiver) = mpsc::channel(200);
        Arbiter::spawn(Consume::new(RateLimiter::new(
            receiver,
            thread_list_rl_config,
        )));

        Ok(Self {
            client,
            last_modified: HashMap::new(),
            media_path,
            media_rl_sender,
            thread_rl_sender,
            thread_list_rl_sender,
            runtime,
        })
    }

    fn fetch_with_last_modified<K: Into<FetchKey>>(
        &mut self,
        uri: Uri,
        key: K,
        ctx: &Context<Self>,
    ) -> impl Future<Item = (hyper::Chunk, DateTime<Utc>), Error = FetchError> {
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
            .and_then(move |res| match res.status() {
                StatusCode::NOT_FOUND => future::err(FetchError::NotFound),
                StatusCode::NOT_MODIFIED => future::err(FetchError::NotModified),
                StatusCode::OK => {
                    let new_modified = res
                        .headers()
                        .get(header::LAST_MODIFIED)
                        .map(|new| {
                            Utc.datetime_from_str(new.to_str().unwrap(), RFC_1123_FORMAT)
                                .unwrap()
                        }).unwrap_or_else(Utc::now);

                    if last_modified > new_modified {
                        warn!(
                            "API sent old data: If-Modified-Since: {}, but Last-Modified: {}",
                            last_modified.format(RFC_1123_FORMAT),
                            new_modified.format(RFC_1123_FORMAT),
                        );
                        future::err(FetchError::NotModified)
                    } else {
                        future::ok((res, new_modified))
                    }
                }
                _ => future::err(res.status().into()),
            }).and_then(move |(res, last_modified)| {
                myself
                    .send(UpdateLastModified(key, last_modified))
                    .then(|_| res.into_body().concat2().from_err())
                    .map(move |body| (body, last_modified))
            })
    }

    fn fetch_media(&mut self, board: Board, filename: String) {
        let mut temp_path = self.media_path.clone();
        temp_path.push(board.to_string());
        temp_path.push("tmp");
        temp_path.push(&filename);

        let mut real_path = self.media_path.clone();
        real_path.push(board.to_string());
        real_path.push(if filename.ends_with("s.jpg") {
            "thumb"
        } else {
            "image"
        });
        real_path.push(&filename[0..4]);
        real_path.push(&filename[4..6]);
        std::fs::create_dir_all(&real_path).unwrap();
        real_path.push(&filename);

        if real_path.exists() {
            error!("/{}/: Media {} already exists!", board, filename);
            return;
        }

        let uri = format!("{}/{}/{}", IMG_URI_PREFIX, board, filename)
            .parse()
            .unwrap_or_else(|err| {
                panic!(
                    "Could not parse URI from ({}, {}): {}",
                    board, filename, err
                );
            });

        let future = self.client.get(uri)
            .from_err()
            .join(tokio::fs::File::create(temp_path.clone()).from_err())
            .and_then(move |(res, file)| match res.status() {
                StatusCode::OK => future::ok((res, file)),
                StatusCode::NOT_FOUND => future::err(FetchError::NotFound),
                _ => future::err(res.status().into()),
            })
            .and_then(|(res, file)| {
                res.into_body().from_err().fold(file, |file, chunk| {
                    tokio::io::write_all(file, chunk)
                        .from_err::<FetchError>()
                        .map(|(file, _)| file)
                })
            }).and_then(move |_| {
                if log_enabled!(Level::Debug) {
                    debug!(
                        "/{}/: Writing {}{}",
                        board,
                        if filename.ends_with("s.jpg") { "" } else { " " },
                        filename
                    );
                }
                tokio::fs::rename(temp_path, real_path).from_err()
            })
            // TODO: Retry request
            .map_err(|err| log_error!(&err));

        self.runtime.spawn(
            self.media_rl_sender
                .clone()
                .send(Box::new(future))
                .map(|_| ())
                .map_err(|err| error!("{}", err)),
        );
    }
}

pub struct RateLimitedResponse<I, E> {
    sender: Sender<Box<Future<Item = (), Error = ()>>>,
    future: Box<Future<Item = I, Error = E>>,
}

impl<A, M, I: 'static, E: 'static> MessageResponse<A, M> for RateLimitedResponse<I, E>
where
    A: Actor,
    M: Message<Result = Result<I, E>>,
{
    fn handle<R: ResponseChannel<M>>(self, _: &mut A::Context, tx: Option<R>) {
        Arbiter::spawn(
            self.sender
                .send(Box::new(self.future.then(move |res| {
                    if let Some(tx) = tx {
                        tx.send(res);
                    }
                    Ok(())
                }))).map(|_| ())
                .map_err(|err| error!("Failed to send RateLimitedResponse future: {}", err)),
        )
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
enum FetchKey {
    Thread(FetchThread),
    Threads(FetchThreads),
}
impl_enum_from!(FetchThread, FetchKey, Thread);
impl_enum_from!(FetchThreads, FetchKey, Threads);

#[derive(Debug, Fail)]
pub enum FetchError {
    #[fail(display = "Hyper error")]
    HyperError(#[cause] hyper::Error),

    #[fail(display = "Bad status: {}", _0)]
    BadStatus(hyper::StatusCode),

    #[fail(display = "JSON error")]
    JsonError(#[cause] serde_json::Error),

    #[fail(display = "Resource not modified")]
    NotModified,

    #[fail(display = "Resource not found")]
    NotFound,

    #[fail(display = "API returned empty data")]
    EmptyData,

    #[fail(display = "Timer error")]
    TimerError(#[cause] tokio::timer::Error),

    #[fail(display = "IO error")]
    IoError(#[cause] std::io::Error),
}
impl_enum_from!(hyper::Error, FetchError, HyperError);
impl_enum_from!(hyper::StatusCode, FetchError, BadStatus);
impl_enum_from!(serde_json::Error, FetchError, JsonError);
impl_enum_from!(tokio::timer::Error, FetchError, TimerError);
impl_enum_from!(std::io::Error, FetchError, IoError);

// We would like to return an ActorFuture from Fetcher, but we can't because ActorFutures can only
// run on their own contexts. So, Fetcher must send a message to itself to update `last_modified`.
#[derive(Message)]
struct UpdateLastModified(FetchKey, DateTime<Utc>);

impl Handler<UpdateLastModified> for Fetcher {
    type Result = ();

    fn handle(&mut self, msg: UpdateLastModified, _ctx: &mut Self::Context) {
        if self
            .last_modified
            .get(&msg.0)
            .map_or(true, |&dt| dt <= msg.1)
        {
            self.last_modified.insert(msg.0, msg.1);
        } else {
            warn!(
                "Ignoring older Last-Modified for {:?}: {} > {}",
                msg.0, self.last_modified[&msg.0], msg.1
            );
        }
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
pub struct FetchThread(pub Board, pub u64);
impl Message for FetchThread {
    type Result = Result<(Vec<Post>, DateTime<Utc>), FetchError>;
}

impl FetchThread {
    fn to_uri(&self) -> Uri {
        format!("{}/{}/thread/{}.json", API_URI_PREFIX, self.0, self.1)
            .parse()
            .unwrap_or_else(|err| {
                panic!("Could not parse URI from {:?}: {}", self, err);
            })
    }
}

impl Handler<FetchThread> for Fetcher {
    type Result = RateLimitedResponse<(Vec<Post>, DateTime<Utc>), FetchError>;

    fn handle(&mut self, msg: FetchThread, ctx: &mut Self::Context) -> Self::Result {
        let future = Box::new(
            self.fetch_with_last_modified(msg.to_uri(), msg, ctx)
                .from_err()
                .and_then(move |(body, last_modified)| {
                    let PostsWrapper { posts } = serde_json::from_slice(&body)?;
                    if posts.is_empty() {
                        Err(FetchError::EmptyData)
                    } else {
                        Ok((posts, last_modified))
                    }
                }),
        );
        RateLimitedResponse {
            sender: self.thread_rl_sender.clone(),
            future,
        }
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
pub struct FetchThreads(pub Board);
impl Message for FetchThreads {
    type Result = Result<(Vec<Thread>, DateTime<Utc>), FetchError>;
}

impl FetchThreads {
    fn to_uri(&self) -> Uri {
        format!("{}/{}/threads.json", API_URI_PREFIX, self.0)
            .parse()
            .unwrap_or_else(|err| {
                panic!("Could not parse URI from {:?}: {}", self, err);
            })
    }
}

impl Handler<FetchThreads> for Fetcher {
    type Result = RateLimitedResponse<(Vec<Thread>, DateTime<Utc>), FetchError>;
    fn handle(&mut self, msg: FetchThreads, ctx: &mut Self::Context) -> Self::Result {
        let future = Box::new(
            self.fetch_with_last_modified(msg.to_uri(), msg, ctx)
                .from_err()
                .and_then(move |(body, last_modified)| {
                    let threads: Vec<ThreadPage> = serde_json::from_slice(&body)?;
                    let mut threads = threads.into_iter().fold(vec![], |mut acc, mut page| {
                        acc.append(&mut page.threads);
                        acc
                    });
                    for (i, thread) in threads.iter_mut().enumerate() {
                        thread.bump_index = i;
                    }
                    if threads.is_empty() {
                        Err(FetchError::EmptyData)
                    } else {
                        Ok((threads, last_modified))
                    }
                }),
        );
        RateLimitedResponse {
            sender: self.thread_list_rl_sender.clone(),
            future,
        }
    }
}

#[derive(Debug)]
pub struct FetchArchive(pub Board);
impl Message for FetchArchive {
    type Result = Result<Vec<u64>, FetchError>;
}

impl FetchArchive {
    fn to_uri(&self) -> Uri {
        format!("{}/{}/archive.json", API_URI_PREFIX, self.0)
            .parse()
            .unwrap_or_else(|err| {
                panic!("Could not parse URI from {:?}: {}", self, err);
            })
    }
}

impl Handler<FetchArchive> for Fetcher {
    type Result = RateLimitedResponse<Vec<u64>, FetchError>;
    fn handle(&mut self, msg: FetchArchive, _ctx: &mut Self::Context) -> Self::Result {
        let future = Box::new(
            self.client
                .get(msg.to_uri())
                .from_err()
                .and_then(move |res| match res.status() {
                    StatusCode::OK => future::ok(res),
                    _ => future::err(res.status().into()),
                }).and_then(|res| res.into_body().concat2().from_err())
                .and_then(move |body| {
                    let archive: Vec<u64> = serde_json::from_slice(&body)?;
                    if archive.is_empty() {
                        Err(FetchError::EmptyData)
                    } else {
                        Ok(archive)
                    }
                }),
        );
        RateLimitedResponse {
            sender: self.thread_list_rl_sender.clone(),
            future,
        }
    }
}

#[derive(Debug, Message)]
pub struct FetchMedia(pub Board, pub Vec<String>);

impl Handler<FetchMedia> for Fetcher {
    type Result = ();
    fn handle(&mut self, msg: FetchMedia, _ctx: &mut Self::Context) {
        let mut temp_path = self.media_path.clone();
        temp_path.push(msg.0.to_string());
        temp_path.push("tmp");
        std::fs::create_dir_all(&temp_path).unwrap();

        for filename in msg.1 {
            self.fetch_media(msg.0, filename);
        }
    }
}

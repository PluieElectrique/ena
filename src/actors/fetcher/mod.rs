use std;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use std::sync::Arc;
use std::time::Duration;

use actix::dev::{MessageResponse, ResponseChannel};
use actix::prelude::*;
use chrono;
use chrono::prelude::*;
use failure::{Error, ResultExt};
use futures::future::{self, Either};
use futures::prelude::*;
use futures::stream;
use futures::sync::mpsc::{self, Sender};
use hyper::client::{Client, HttpConnector};
use hyper::header::{self, HeaderValue};
use hyper::{self, Body, Request, StatusCode, Uri};
use hyper_tls::HttpsConnector;
use serde_json;
use tokio;
use tokio::runtime::Runtime;

mod rate_limiter;

use self::rate_limiter::{Consume, RateLimiter};
use four_chan::*;
use RateLimitingConfig;

type HttpsClient = Client<HttpsConnector<HttpConnector>>;

const RFC_1123_FORMAT: &str = "%a, %d %b %Y %T GMT";
const MEDIA_CHANNEL_CAPACITY: usize = 1000;
const THREAD_CHANNEL_CAPACITY: usize = 500;
const THREAD_LIST_CHANNEL_CAPACITY: usize = 200;

/// An actor which fetches threads, thread lists, archives, and media from the 4chan API. Fetching
/// the catalog or pages of a board or boards.json is not used and thus unsupported.
pub struct Fetcher {
    client: Arc<HttpsClient>,
    last_modified: HashMap<LastModifiedKey, DateTime<Utc>>,
    media_rl_sender: Sender<FetchMedia>,
    thread_rl_sender: Sender<Box<Future<Item = (), Error = ()>>>,
    thread_list_rl_sender: Sender<Box<Future<Item = (), Error = ()>>>,
    // Fetcher must use its own runtime for fetching media because tokio::fs functions can't use the
    // current_thread runtime that Actix provides
    runtime: Runtime,
}

impl Actor for Fetcher {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(500);
        // Clean up old Last-Modified values so that we don't leak memory
        ctx.run_interval(Duration::from_secs(86400), |act, _ctx| {
            let yesterday = Utc::now() - chrono::Duration::days(1);
            act.last_modified.retain(|_key, &mut dt| dt > yesterday);
        });
    }
}

impl Fetcher {
    pub fn new(
        media_path: &Path,
        media_rl_config: &RateLimitingConfig,
        thread_rl_config: &RateLimitingConfig,
        thread_list_rl_config: &RateLimitingConfig,
    ) -> Result<Self, Error> {
        std::fs::create_dir_all(media_path).context("Could not create media directory")?;
        {
            let mut test_file = media_path.to_owned();
            test_file.push("ena_permission_test");
            std::fs::File::create(&test_file).context("Could not access media directory")?;
            std::fs::remove_file(&test_file)
                .context("Could not remove media directory permission test file")?;
        }

        let mut runtime = Runtime::new().unwrap();
        let https = HttpsConnector::new(2).context("Could not create HttpsConnector")?;
        let client = Arc::new(Client::builder().build::<_, Body>(https));

        let media_rl_sender = {
            let (sender, receiver) = mpsc::channel(MEDIA_CHANNEL_CAPACITY);
            let client = client.clone();
            let media_path = media_path.to_owned();
            let stream = receiver
                .map(|FetchMedia(board, filenames)| {
                    stream::iter_ok(filenames.into_iter().map(move |filename| (board, filename)))
                }).flatten()
                .map(move |(board, filename)| {
                    fetch_media(board, filename, &client, media_path.clone())
                });
            runtime.spawn(Consume::new(RateLimiter::new(stream, media_rl_config)));
            sender
        };

        let (thread_rl_sender, receiver) = mpsc::channel(THREAD_CHANNEL_CAPACITY);
        Arbiter::spawn(Consume::new(RateLimiter::new(receiver, thread_rl_config)));

        let (thread_list_rl_sender, receiver) = mpsc::channel(THREAD_LIST_CHANNEL_CAPACITY);
        Arbiter::spawn(Consume::new(RateLimiter::new(
            receiver,
            thread_list_rl_config,
        )));

        Ok(Self {
            client,
            last_modified: HashMap::new(),
            media_rl_sender,
            thread_rl_sender,
            thread_list_rl_sender,
            runtime,
        })
    }

    fn fetch_with_last_modified<'a, M: 'a>(
        &mut self,
        msg: &'a M,
        ctx: &Context<Self>,
    ) -> impl Future<Item = (hyper::Chunk, DateTime<Utc>), Error = FetchError>
    where
        &'a M: ToUri + Into<LastModifiedKey>,
    {
        let uri = msg.to_uri();
        let mut request = Request::get(uri.clone()).body(Body::default()).unwrap();
        let key = msg.into();
        let myself = ctx.address();

        let last_modified = self
            .last_modified
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Utc.timestamp(1_065_062_160, 0));
        {
            let headers = request.headers_mut();
            headers.reserve(1);
            headers.insert(
                header::IF_MODIFIED_SINCE,
                HeaderValue::from_str(last_modified.format(RFC_1123_FORMAT).to_string().as_str())
                    .unwrap(),
            );
        }
        let client = self.client.clone();
        future::lazy(move || client.request(request))
            .from_err()
            .and_then(move |res| match res.status() {
                StatusCode::NOT_FOUND => Err(FetchError::NotFound(uri)),
                StatusCode::NOT_MODIFIED => Err(FetchError::NotModified),
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
                        Err(FetchError::NotModified)
                    } else {
                        Ok((res, new_modified))
                    }
                }
                _ => Err(res.status().into()),
            }).and_then(move |(res, last_modified)| {
                myself
                    .send(UpdateLastModified(key, last_modified))
                    .from_err()
                    .and_then(|_| res.into_body().concat2().from_err())
                    .map(move |body| (body, last_modified))
            })
    }
}

/// `(board, Some(no))` represents a thread and `(board, None)` represents the `threads.json` of that board.
type LastModifiedKey = (Board, Option<u64>);

impl<'a> From<&'a FetchThread> for LastModifiedKey {
    fn from(msg: &FetchThread) -> Self {
        (msg.0, Some(msg.1))
    }
}

impl<'a> From<&'a FetchThreadList> for LastModifiedKey {
    fn from(msg: &FetchThreadList) -> Self {
        (msg.0, None)
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

#[derive(Debug, Fail)]
pub enum FetchError {
    #[fail(display = "Hyper error: {}", _0)]
    HyperError(hyper::Error),

    #[fail(display = "Bad status: {}", _0)]
    BadStatus(hyper::StatusCode),

    #[fail(display = "JSON error: {}", _0)]
    JsonError(serde_json::Error),

    #[fail(display = "Resource not modified")]
    NotModified,

    #[fail(display = "Resource not found: {}", _0)]
    NotFound(Uri),

    #[fail(display = "API returned empty data")]
    EmptyData,

    #[fail(display = "Timer error: {}", _0)]
    TimerError(tokio::timer::Error),

    #[fail(display = "IO error: {}", _0)]
    IoError(std::io::Error),

    #[fail(display = "Mailbox error: {}", _0)]
    MailboxError(MailboxError),
}
impl_enum_from!(hyper::Error, FetchError, HyperError);
impl_enum_from!(hyper::StatusCode, FetchError, BadStatus);
impl_enum_from!(serde_json::Error, FetchError, JsonError);
impl_enum_from!(tokio::timer::Error, FetchError, TimerError);
impl_enum_from!(std::io::Error, FetchError, IoError);
impl_enum_from!(MailboxError, FetchError, MailboxError);

trait ToUri {
    fn to_uri(&self) -> Uri;
}

// We would like to return an ActorFuture from Fetcher, but we can't because ActorFutures can only
// run on their own contexts. So, Fetcher must send a message to itself to update `last_modified`.
struct UpdateLastModified(LastModifiedKey, DateTime<Utc>);
impl Message for UpdateLastModified {
    type Result = Result<(), FetchError>;
}

impl Handler<UpdateLastModified> for Fetcher {
    type Result = Result<(), FetchError>;

    fn handle(&mut self, msg: UpdateLastModified, _ctx: &mut Self::Context) -> Self::Result {
        if self
            .last_modified
            .get(&msg.0)
            .map_or(true, |&dt| dt <= msg.1)
        {
            self.last_modified.insert(msg.0, msg.1);
            Ok(())
        } else {
            error!(
                "Ignoring older Last-Modified for {:?}: {} > {}",
                msg.0, self.last_modified[&msg.0], msg.1
            );
            Err(FetchError::NotModified)
        }
    }
}

pub struct FetchThread(pub Board, pub u64);
impl Message for FetchThread {
    type Result = Result<(Vec<Post>, DateTime<Utc>), FetchError>;
}

impl<'a> ToUri for &'a FetchThread {
    fn to_uri(&self) -> Uri {
        format!("{}/{}/thread/{}.json", API_URI_PREFIX, self.0, self.1)
            .parse()
            .unwrap()
    }
}

impl Handler<FetchThread> for Fetcher {
    type Result = RateLimitedResponse<(Vec<Post>, DateTime<Utc>), FetchError>;

    fn handle(&mut self, msg: FetchThread, ctx: &mut Self::Context) -> Self::Result {
        let future = Box::new(self.fetch_with_last_modified(&msg, ctx).from_err().and_then(
            move |(body, last_modified)| {
                let PostsWrapper { posts } = serde_json::from_slice(&body)?;
                if posts.is_empty() {
                    Err(FetchError::EmptyData)
                } else {
                    Ok((posts, last_modified))
                }
            },
        ));
        RateLimitedResponse {
            sender: self.thread_rl_sender.clone(),
            future,
        }
    }
}

pub struct FetchThreadList(pub Board);
impl Message for FetchThreadList {
    type Result = Result<(Vec<Thread>, DateTime<Utc>), FetchError>;
}

impl<'a> ToUri for &'a FetchThreadList {
    fn to_uri(&self) -> Uri {
        format!("{}/{}/threads.json", API_URI_PREFIX, self.0)
            .parse()
            .unwrap()
    }
}

impl Handler<FetchThreadList> for Fetcher {
    type Result = RateLimitedResponse<(Vec<Thread>, DateTime<Utc>), FetchError>;
    fn handle(&mut self, msg: FetchThreadList, ctx: &mut Self::Context) -> Self::Result {
        let future = Box::new(self.fetch_with_last_modified(&msg, ctx).from_err().and_then(
            move |(body, last_modified)| {
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
            },
        ));
        RateLimitedResponse {
            sender: self.thread_list_rl_sender.clone(),
            future,
        }
    }
}

pub struct FetchArchive(pub Board);
impl Message for FetchArchive {
    type Result = Result<Vec<u64>, FetchError>;
}

impl ToUri for FetchArchive {
    fn to_uri(&self) -> Uri {
        format!("{}/{}/archive.json", API_URI_PREFIX, self.0)
            .parse()
            .unwrap()
    }
}

impl Handler<FetchArchive> for Fetcher {
    type Result = RateLimitedResponse<Vec<u64>, FetchError>;
    fn handle(&mut self, msg: FetchArchive, _ctx: &mut Self::Context) -> Self::Result {
        assert!(msg.0.is_archived());
        let client = self.client.clone();
        let request = Request::get(msg.to_uri()).body(Body::default()).unwrap();
        let future = Box::new(
            future::lazy(move || client.request(request))
                .from_err()
                .and_then(move |res| match res.status() {
                    StatusCode::OK => Ok(res),
                    _ => Err(res.status().into()),
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

#[derive(Message)]
pub struct FetchMedia(pub Board, pub Vec<String>);

impl Handler<FetchMedia> for Fetcher {
    type Result = ();
    fn handle(&mut self, msg: FetchMedia, _ctx: &mut Self::Context) {
        // If a media future panics, the media runtime will crash and the sender will close. The
        // Actix system has its own runtime, so it won't crash. But, we can't recover from a media
        // runtime panic, so if the media runtime crashes we crash the Actix system as well.
        if self.media_rl_sender.is_closed() {
            panic!("Media sender is closed");
        }

        self.runtime.spawn(
            self.media_rl_sender
                .clone()
                .send(msg)
                .map(|_| ())
                .map_err(|err| error!("{}", err)),
        );
    }
}

fn fetch_media(
    board: Board,
    filename: String,
    client: &Arc<HttpsClient>,
    media_path: PathBuf,
) -> impl Future<Item = (), Error = ()> {
    let is_thumb = filename.ends_with("s.jpg");

    let mut temp_path = media_path.clone();
    temp_path.push(board.to_string());
    temp_path.push("tmp");
    let temp_dir_future = tokio::fs::create_dir_all(temp_path.clone());
    temp_path.push(&filename);
    let temp_file_future = tokio::fs::File::create(temp_path.clone());

    let mut real_path = media_path;
    real_path.push(board.to_string());
    real_path.push(if is_thumb { "thumb" } else { "image" });
    real_path.push(&filename[0..4]);
    real_path.push(&filename[4..6]);
    let real_dir_future = tokio::fs::create_dir_all(real_path.clone());
    real_path.push(&filename);

    if real_path.exists() {
        error!("/{}/: Media {} already exists!", board, filename);
        return Either::A(future::err(()));
    }

    let uri: Uri = format!("{}/{}/{}", IMG_URI_PREFIX, board, filename)
        .parse()
        .unwrap_or_else(|err| {
            panic!(
                "Could not parse URI from ({}, {}): {}",
                board, filename, err
            );
        });

    let future = client
        .get(uri.clone())
        .from_err()
        .join3(
            temp_dir_future.and_then(|_| temp_file_future).from_err(),
            real_dir_future.from_err(),
        ).and_then(move |(res, file, _)| match res.status() {
            StatusCode::OK => Ok((res, file)),
            StatusCode::NOT_FOUND => Err(FetchError::NotFound(uri)),
            _ => Err(res.status().into()),
        }).and_then(|(res, file)| {
            res.into_body().from_err().fold(file, |file, chunk| {
                tokio::io::write_all(file, chunk)
                    .from_err::<FetchError>()
                    .map(|(file, _)| file)
            })
        }).and_then(move |_| {
            debug!(
                "/{}/: Writing {}{}",
                board,
                if is_thumb { "" } else { " " },
                filename
            );
            tokio::fs::rename(temp_path, real_path).from_err()
        }).map_err(move |err| error!("/{}/: Failed to fetch media: {}", board, err));

    Either::B(future)
}

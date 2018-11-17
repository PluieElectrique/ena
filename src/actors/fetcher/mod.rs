use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use actix::dev::ResponseChannel;
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

use actors::ThreadUpdater;
use config::RateLimitingSettings;
use four_chan::*;

mod delay_queue;
mod error;
mod helper;
mod messages;
mod rate_limiter;

pub use self::error::FetchError;
pub use self::messages::*;

use self::delay_queue::DelayQueue;
use self::helper::*;
use self::rate_limiter::StreamExt;

type HttpsClient = Client<HttpsConnector<HttpConnector>>;

const RFC_1123_FORMAT: &str = "%a, %d %b %Y %T GMT";

const FETCHER_MAILBOX_CAPACITY: usize = 500;

const MEDIA_CHANNEL_CAPACITY: usize = 1000;
const THREAD_CHANNEL_CAPACITY: usize = 500;
const THREAD_LIST_CHANNEL_CAPACITY: usize = 200;

/// Request retry base delay in seconds.
const REQUEST_RETRY_DELAY: u64 = 2;
/// Request retry exponential delay multiplier. (d, d * m, d * m^2, d * m^3, ...)
const REQUEST_RETRY_FACTOR: u64 = 2;
/// Request retry maximum delay.
const REQUEST_RETRY_MAX_DELAY: u64 = 256;

/// An actor which fetches threads, thread lists, archives, and media from the 4chan API.
///
/// Fetching the catalog or pages of a board or `boards.json` is not used and thus unsupported.
pub struct Fetcher {
    client: Arc<HttpsClient>,
    last_modified: HashMap<LastModifiedKey, DateTime<Utc>>,
    media_sender: Sender<FetchMedia>,
    thread_sender: Sender<(FetchThreads, Vec<DateTime<Utc>>)>,
    thread_list_sender: Sender<Box<Future<Item = (), Error = ()>>>,
    // Fetcher must use its own runtime for fetching media because tokio::fs functions can't use the
    // current_thread runtime that Actix provides
    runtime: Runtime,
}

impl Actor for Fetcher {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // Clean up old Last-Modified values so that we don't leak memory
        ctx.run_interval(Duration::from_secs(86400), |act, _ctx| {
            let yesterday = Utc::now() - chrono::Duration::days(1);
            act.last_modified.retain(|_key, &mut dt| dt > yesterday);
        });
    }
}

impl Fetcher {
    pub fn create(
        media_path: &Path,
        media_rl_settings: &RateLimitingSettings,
        thread_rl_settings: &RateLimitingSettings,
        thread_list_rl_settings: &RateLimitingSettings,
        thread_updater: Addr<ThreadUpdater>,
    ) -> Result<Addr<Self>, Error> {
        let ctx = {
            let (_, receiver) = actix::dev::channel::channel(FETCHER_MAILBOX_CAPACITY);
            Context::with_receiver(receiver)
        };
        let fetcher = Fetcher::new(
            media_path,
            media_rl_settings,
            thread_rl_settings,
            thread_list_rl_settings,
            thread_updater,
            ctx.address(),
        )?;
        Ok(ctx.run(fetcher))
    }

    fn new(
        media_path: &Path,
        media_rl_settings: &RateLimitingSettings,
        thread_rl_settings: &RateLimitingSettings,
        thread_list_rl_settings: &RateLimitingSettings,
        thread_updater: Addr<ThreadUpdater>,
        fetcher: Addr<Self>,
    ) -> Result<Self, Error> {
        let mut runtime = Runtime::new().unwrap();
        let https = HttpsConnector::new(2).context("Could not create HttpsConnector")?;
        let client = Arc::new(Client::builder().build::<_, Body>(https));

        let media_sender = {
            let (sender, receiver) = mpsc::channel(MEDIA_CHANNEL_CAPACITY);
            let client = client.clone();
            let media_path = media_path.to_owned();

            let (retry_sender, retry_receiver) = mpsc::channel(MEDIA_CHANNEL_CAPACITY);

            let future = receiver
                .map(|FetchMedia(board, filenames)| {
                    stream::iter_ok(
                        filenames
                            .into_iter()
                            .map(move |filename| (board, filename, REQUEST_RETRY_DELAY)),
                    )
                }).flatten()
                .select(DelayQueue::new(retry_receiver))
                .map(move |request| {
                    fetch_media(request, &client, media_path.clone(), retry_sender.clone())
                }).rate_limit(media_rl_settings)
                .consume();
            runtime.spawn(future);
            sender
        };

        let thread_sender = {
            let (sender, receiver) = mpsc::channel(THREAD_CHANNEL_CAPACITY);
            let client = client.clone();
            let future = receiver
                .map(|(msg, last_modified): (FetchThreads, Vec<DateTime<Utc>>)| {
                    let FetchThreads(board, nums, from_archive_json) = msg;
                    stream::iter_ok(nums.into_iter().zip(last_modified.into_iter())).map(
                        move |(no, last_modified)| {
                            (FetchThread(board, no, from_archive_json), last_modified)
                        },
                    )
                }).flatten()
                .map(move |(msg, last_modified)| {
                    fetch_thread(
                        msg,
                        last_modified,
                        &client,
                        fetcher.clone(),
                        thread_updater.clone(),
                    )
                }).rate_limit(thread_rl_settings)
                .consume();
            Arbiter::spawn(future);
            sender
        };

        let thread_list_sender = {
            let (sender, receiver) = mpsc::channel(THREAD_LIST_CHANNEL_CAPACITY);
            Arbiter::spawn(receiver.rate_limit(thread_list_rl_settings).consume());
            sender
        };

        Ok(Self {
            client,
            last_modified: HashMap::new(),
            media_sender,
            thread_sender,
            thread_list_sender,
            runtime,
        })
    }

    fn get_last_modified<'a, K: 'a>(&self, key: &'a K) -> DateTime<Utc>
    where
        &'a K: Into<LastModifiedKey>,
    {
        self.last_modified
            .get(&key.into())
            .cloned()
            .unwrap_or_else(|| Utc.timestamp(1_065_062_160, 0))
    }
}

fn fetch_with_last_modified<'a, M: 'a>(
    msg: &'a M,
    last_modified: DateTime<Utc>,
    client: &Arc<HttpsClient>,
    fetcher: Addr<Fetcher>,
) -> impl Future<Item = (hyper::Chunk, DateTime<Utc>), Error = FetchError>
where
    &'a M: ToUri + Into<LastModifiedKey>,
{
    let uri = msg.to_uri();
    let key = msg.into();

    let mut request = Request::get(uri.clone()).body(Body::default()).unwrap();
    {
        let headers = request.headers_mut();
        headers.reserve(1);
        headers.insert(
            header::IF_MODIFIED_SINCE,
            HeaderValue::from_str(last_modified.format(RFC_1123_FORMAT).to_string().as_str())
                .unwrap(),
        );
    }

    client
        .request(request)
        .from_err()
        .and_then(move |res| match res.status() {
            StatusCode::NOT_FOUND => Err(FetchError::NotFound(uri.to_string())),
            StatusCode::NOT_MODIFIED => Err(FetchError::NotModified),
            StatusCode::OK => {
                let new_modified = res
                    .headers()
                    .get(header::LAST_MODIFIED)
                    .map(|new| {
                        Utc.datetime_from_str(new.to_str().unwrap(), RFC_1123_FORMAT)
                            .unwrap_or_else(|err| {
                                error!("Could not parse Last-Modified header: {}", err);
                                Utc::now()
                            })
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
            fetcher
                .send(UpdateLastModified(key, last_modified))
                .from_err()
                .and_then(|_| res.into_body().concat2().from_err())
                .map(move |body| (body, last_modified))
        })
}

pub struct FetchThread(pub Board, pub u64, pub bool);

impl<'a> ToUri for &'a FetchThread {
    fn to_uri(&self) -> Uri {
        format!("{}/{}/thread/{}.json", API_URI_PREFIX, self.0, self.1)
            .parse()
            .unwrap()
    }
}

fn fetch_thread(
    msg: FetchThread,
    last_modified: DateTime<Utc>,
    client: &Arc<HttpsClient>,
    fetcher: Addr<Fetcher>,
    thread_updater: Addr<ThreadUpdater>,
) -> impl Future<Item = (), Error = ()> {
    fetch_with_last_modified(&msg, last_modified, client, fetcher)
        .and_then(move |(body, last_modified)| {
            let PostsWrapper { posts } = serde_json::from_slice(&body)?;
            if posts.is_empty() {
                Err(FetchError::EmptyData)
            } else {
                Ok((posts, last_modified))
            }
        }).then(move |result| {
            let reply = FetchedThread {
                request: msg,
                result,
            };
            thread_updater.send(reply).map_err(|err| log_error!(&err))
        })
}

fn fetch_media(
    (board, filename, delay): (Board, String, u64),
    client: &Arc<HttpsClient>,
    media_path: PathBuf,
    retry_sender: Sender<((Board, String, u64), Duration)>,
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
            StatusCode::NOT_FOUND => Err(FetchError::NotFound(uri.to_string())),
            _ => Err(res.status().into()),
        }).and_then(|(res, file)| {
            res.into_body().from_err().fold(file, |file, chunk| {
                tokio::io::write_all(file, chunk)
                    .from_err::<FetchError>()
                    .map(|(file, _)| file)
            })
        }).and_then({
            let filename = filename.clone();
            move |_| {
                debug!(
                    "/{}/: Writing {}{}",
                    board,
                    if is_thumb { "" } else { " " },
                    filename
                );
                tokio::fs::rename(temp_path, real_path).from_err()
            }
        }).then(move |res| {
            use self::FetchError::*;
            let should_retry = |err: &FetchError| match *err {
                NotFound(_) => false,
                EmptyData | JsonError(_) | NotModified => unreachable!(),
                _ => true,
            };

            if let Err(err) = res {
                if should_retry(&err) && delay <= REQUEST_RETRY_MAX_DELAY {
                    error!(
                        "/{}/: Failed to fetch media {}, retrying: {}",
                        board, filename, err
                    );
                    let duration = Duration::from_secs(delay);
                    return Either::B(
                        retry_sender
                            .send(((board, filename, delay * REQUEST_RETRY_FACTOR), duration))
                            .map(|_| ())
                            .map_err(|err| error!("{}", err)),
                    );
                } else {
                    error!(
                        "/{}/: Failed to fetch media {}, not retrying: {}",
                        board, filename, err
                    );
                }
            }

            Either::A(future::ok(()))
        });

    Either::B(future)
}

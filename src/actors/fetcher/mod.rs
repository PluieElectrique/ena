use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use actix::{dev::ResponseChannel, prelude::*};
use chrono::prelude::*;
use failure::{Error, ResultExt};
use futures::{
    future::{self, Either},
    prelude::*,
    stream,
    sync::mpsc::{self, Sender},
};
use hyper::{
    client::HttpConnector,
    header::{self, HeaderValue},
    Body, Client, Request, StatusCode, Uri,
};
use hyper_tls::HttpsConnector;
use tokio::runtime::Runtime;

use super::thread_updater::{FetchedThread, ThreadUpdater};
use crate::{config::Config, four_chan::*};

mod error;
mod helper;
mod messages;
mod rate_limiter;
mod retry;

pub use {error::FetchError, messages::*};
use {helper::*, rate_limiter::StreamExt, retry::Retry};

type HttpsClient = Client<HttpsConnector<HttpConnector>>;

const RFC_1123_FORMAT: &str = "%a, %d %b %Y %T GMT";

const FETCHER_MAILBOX_CAPACITY: usize = 500;

const MEDIA_CHANNEL_CAPACITY: usize = 1000;
const THREAD_CHANNEL_CAPACITY: usize = 500;
const THREAD_LIST_CHANNEL_CAPACITY: usize = 200;

/// An actor which fetches threads, thread lists, archives, and media from the 4chan API.
///
/// Fetching the catalog or pages of a board or `boards.json` is not used and thus unsupported.
pub struct Fetcher {
    client: Arc<HttpsClient>,
    last_modified: HashMap<LastModifiedKey, DateTime<Utc>>,
    media_sender: Sender<FetchMedia>,
    thread_sender: Sender<(FetchThreads, Vec<DateTime<Utc>>)>,
    thread_list_sender: Sender<Box<dyn Future<Item = (), Error = ()>>>,
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
    /// Creates and starts a new `Fetcher` actor.
    // We don't let the caller start the actor themselves because Fetcher needs to hold its own
    // address. To do this, we first create a Context, which gives us Addr<Fetcher> without having
    // to create Fetcher. Then, we pass this Addr into new(). Finally, we start the resulting actor
    // with the previously created Context.
    pub fn create(
        config: &Config,
        thread_updater: Addr<ThreadUpdater>,
    ) -> Result<Addr<Self>, Error> {
        let ctx = {
            let (_, receiver) = actix::dev::channel::channel(FETCHER_MAILBOX_CAPACITY);
            Context::with_receiver(receiver)
        };
        let fetcher = Fetcher::try_new(config, thread_updater, ctx.address())?;
        Ok(ctx.run(fetcher))
    }

    fn try_new(
        config: &Config,
        thread_updater: Addr<ThreadUpdater>,
        fetcher: Addr<Self>,
    ) -> Result<Self, Error> {
        let mut runtime = Runtime::new().unwrap();
        let https = HttpsConnector::new(1).context("Could not create HttpsConnector")?;
        let client = Arc::new(Client::builder().build::<_, Body>(https));

        let media_sender = {
            let (sender, receiver) = mpsc::channel(MEDIA_CHANNEL_CAPACITY);
            let client = client.clone();
            let media_path = config.database_media.media_path.to_owned();

            let (retry_sender, retry_receiver) = retry::retry_channel(MEDIA_CHANNEL_CAPACITY);
            let retry_backoff = config.network.retry_backoff;

            let future = receiver
                .map(|FetchMedia(board, filenames)| {
                    stream::iter_ok(filenames.into_iter().map(move |filename| (board, filename)))
                })
                .flatten()
                .map(move |request| Retry::new(request, &retry_backoff))
                .select(retry_receiver)
                .map(move |retry| {
                    fetch_media_retry(retry, &client, media_path.clone(), retry_sender.clone())
                })
                .rate_limit(&config.network.rate_limiting.media)
                .consume();
            runtime.spawn(future);
            sender
        };

        let thread_sender = {
            let (sender, receiver) = mpsc::channel(THREAD_CHANNEL_CAPACITY);
            let client = client.clone();

            let (retry_sender, retry_receiver) = retry::retry_channel(THREAD_CHANNEL_CAPACITY);
            let retry_backoff = config.network.retry_backoff;

            let future = receiver
                .map(|(msg, last_modified): (FetchThreads, Vec<DateTime<Utc>>)| {
                    let FetchThreads(board, nums, from_archive_json) = msg;
                    stream::iter_ok(nums.into_iter().zip(last_modified.into_iter())).map(
                        move |(no, last_modified)| {
                            (FetchThread(board, no, from_archive_json), last_modified)
                        },
                    )
                })
                .flatten()
                .map(move |request| Retry::new(request, &retry_backoff))
                .select(retry_receiver)
                .map(move |retry| {
                    fetch_thread_retry(
                        retry,
                        &client,
                        fetcher.clone(),
                        thread_updater.clone(),
                        retry_sender.clone(),
                    )
                })
                .rate_limit(&config.network.rate_limiting.thread)
                .consume();
            Arbiter::spawn(future);
            sender
        };

        let thread_list_sender = {
            let (sender, receiver) = mpsc::channel(THREAD_LIST_CHANNEL_CAPACITY);
            Arbiter::spawn(
                receiver
                    .rate_limit(&config.network.rate_limiting.thread_list)
                    .consume(),
            );
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

fn fetch_with_last_modified<'a, R: 'a>(
    request: &'a R,
    last_modified: DateTime<Utc>,
    client: &Arc<HttpsClient>,
    fetcher: Addr<Fetcher>,
) -> impl Future<Item = (hyper::Chunk, DateTime<Utc>), Error = FetchError>
where
    &'a R: ToUri + Into<LastModifiedKey>,
{
    let uri = request.to_uri();
    let key = request.into();

    let mut request = Request::get(uri.clone()).body(Body::default()).unwrap();
    let headers = request.headers_mut();
    headers.reserve(1);
    headers.insert(
        header::IF_MODIFIED_SINCE,
        HeaderValue::from_str(last_modified.format(RFC_1123_FORMAT).to_string().as_str()).unwrap(),
    );

    client
        .request(request)
        .from_err()
        .and_then(move |res| match res.status() {
            StatusCode::NOT_FOUND => Err(FetchError::NotFound(uri.to_string())),
            StatusCode::NOT_MODIFIED => Err(FetchError::NotModified),
            StatusCode::OK => {
                let new_modified =
                    res.headers()
                        .get(header::LAST_MODIFIED)
                        .map_or_else(Utc::now, |h| {
                            h.to_str()
                                .map(|h| Utc.datetime_from_str(h, RFC_1123_FORMAT))
                                .unwrap_or_else(|err| {
                                    error!("Could not parse Last-Modified header: {}", err);
                                    Ok(Utc::now())
                                })
                                .unwrap_or_else(|err| {
                                    error!("Could not parse Last-Modified header: {}", err);
                                    Utc::now()
                                })
                        });

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
        })
        .and_then(move |(res, last_modified)| {
            fetcher
                .send(UpdateLastModified(key, last_modified))
                .from_err()
                .and_then(|_| res.into_body().concat2().from_err())
                .map(move |body| (body, last_modified))
        })
}

#[derive(Clone, Copy)]
pub struct FetchThread(pub Board, pub u64, pub bool);

impl ToUri for &FetchThread {
    fn to_uri(&self) -> Uri {
        format!("{}/{}/thread/{}.json", API_URI_PREFIX, self.0, self.1)
            .parse()
            .unwrap()
    }
}

fn fetch_thread(
    request: (FetchThread, DateTime<Utc>),
    client: &Arc<HttpsClient>,
    fetcher: Addr<Fetcher>,
) -> impl Future<Item = (Vec<Post>, DateTime<Utc>), Error = FetchError> {
    fetch_with_last_modified(&request.0, request.1, client, fetcher).and_then(
        move |(body, last_modified)| {
            let PostsWrapper { posts } = serde_json::from_slice(&body)?;
            if posts.is_empty() {
                Err(FetchError::EmptyThread)
            } else if posts[0].reply_to != 0 || posts.iter().skip(1).any(|p| p.reply_to == 0) {
                Err(FetchError::InvalidReplyTo)
            } else {
                Ok((posts, last_modified))
            }
        },
    )
}

fn fetch_thread_retry(
    retry: Retry<(FetchThread, DateTime<Utc>)>,
    client: &Arc<HttpsClient>,
    fetcher: Addr<Fetcher>,
    thread_updater: Addr<ThreadUpdater>,
    retry_sender: Sender<Retry<(FetchThread, DateTime<Utc>)>>,
) -> impl Future<Item = (), Error = ()> {
    fetch_thread(retry.to_data(), client, fetcher).then(move |result| {
        use FetchError::*;
        if let Err(ref err) = result {
            let will_retry = retry.can_retry()
                && match err {
                    NotFound(_) | NotModified => false,
                    ExistingMedia => unreachable!(),
                    _ => true,
                };

            if will_retry {
                let &(FetchThread(board, no, _), _) = retry.as_data();
                error!("/{}/ No. {}: Failed to fetch, retrying: {}", board, no, err);
                return Either::A(
                    retry_sender
                        .send(retry)
                        .map(|_| ())
                        .map_err(|err| error!("{}", err)),
                );
            }
        }
        let reply = FetchedThread {
            request: retry.into_data().0,
            result,
        };
        Either::B(thread_updater.send(reply).map_err(|err| log_error!(&err)))
    })
}

fn fetch_thread_list(
    msg: &FetchThreadList,
    last_modified: DateTime<Utc>,
    client: &Arc<HttpsClient>,
    fetcher: Addr<Fetcher>,
) -> Box<dyn Future<Item = (Vec<Thread>, DateTime<Utc>), Error = FetchError>> {
    Box::new(
        fetch_with_last_modified(msg, last_modified, client, fetcher)
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
                Ok((threads, last_modified))
            }),
    )
}

fn fetch_archive(
    msg: &FetchArchive,
    client: &Arc<HttpsClient>,
) -> Box<dyn Future<Item = Vec<u64>, Error = FetchError>> {
    assert!(msg.0.is_archived());
    Box::new(
        client
            .get(msg.to_uri())
            .from_err()
            .and_then(move |res| match res.status() {
                StatusCode::OK => Ok(res),
                _ => Err(res.status().into()),
            })
            .and_then(|res| res.into_body().concat2().from_err())
            .and_then(move |body| {
                let archive: Vec<u64> = serde_json::from_slice(&body)?;
                Ok(archive)
            }),
    )
}

fn fetch_media(
    (board, filename): (Board, String),
    client: &Arc<HttpsClient>,
    media_path: PathBuf,
) -> impl Future<Item = (), Error = FetchError> {
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
        return Either::A(future::err(FetchError::ExistingMedia));
    }

    let uri: Uri = match format!("{}/{}/{}", IMG_URI_PREFIX, board, filename).parse() {
        Ok(uri) => uri,
        Err(err) => return Either::A(future::err(err.into())),
    };

    let future = client
        .get(uri.clone())
        .from_err()
        .join3(
            temp_dir_future.and_then(|_| temp_file_future).from_err(),
            real_dir_future.from_err(),
        )
        .and_then(move |(res, file, _)| match res.status() {
            StatusCode::OK => Ok((res, file)),
            StatusCode::NOT_FOUND => Err(FetchError::NotFound(uri.to_string())),
            _ => Err(res.status().into()),
        })
        .and_then(|(res, file)| {
            res.into_body().from_err().fold(file, |file, chunk| {
                tokio::io::write_all(file, chunk)
                    .from_err::<FetchError>()
                    .map(|(file, _)| file)
            })
        })
        .and_then({
            let filename = filename.clone();
            move |_| {
                debug!(
                    "/{}/: Fetched {}{}",
                    board,
                    if is_thumb { "" } else { " " },
                    filename
                );
                tokio::fs::rename(temp_path, real_path).from_err()
            }
        });
    Either::B(future)
}

fn fetch_media_retry(
    retry: Retry<(Board, String)>,
    client: &Arc<HttpsClient>,
    media_path: PathBuf,
    retry_sender: Sender<Retry<(Board, String)>>,
) -> impl Future<Item = (), Error = ()> {
    fetch_media(retry.to_data(), client, media_path).or_else(move |err| {
        use FetchError::*;
        let will_retry = retry.can_retry()
            && match err {
                ExistingMedia | NotFound(_) => false,
                EmptyThread | InvalidReplyTo | JsonError(_) | NotModified => unreachable!(),
                _ => true,
            };

        let &(board, ref filename) = retry.as_data();
        error!(
            "/{}/: Failed to fetch {}{}: {}",
            board,
            filename,
            if will_retry { ", retrying" } else { "" },
            err
        );

        if will_retry {
            Either::A(
                retry_sender
                    .send(retry)
                    .map(|_| ())
                    .map_err(|err| error!("{}", err)),
            )
        } else {
            Either::B(future::ok(()))
        }
    })
}

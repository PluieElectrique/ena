use super::*;

// The only way to update `last_modified` would be to use an ActorFuture. But, Fetcher sends its
// futures to RateLimiters, which prevent ActorFutures from being used. So, Fetcher must send a
// message to itself to update `last_modified`.
pub struct UpdateLastModified(pub LastModifiedKey, pub DateTime<Utc>);
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

#[derive(Message)]
pub struct FetchThreads(pub Board, pub Vec<u64>, pub bool);

impl Handler<FetchThreads> for Fetcher {
    type Result = ();

    fn handle(&mut self, msg: FetchThreads, _: &mut Self::Context) {
        let board = msg.0;
        let last_modified = msg
            .1
            .iter()
            .map(|&no| self.get_last_modified(&(board, no)))
            .collect();

        Arbiter::spawn(
            self.thread_rl_sender
                .clone()
                .send((msg, last_modified))
                .map(|_| ())
                .map_err(|err| error!("{}", err)),
        );
    }
}

#[derive(Message)]
pub struct FetchedThread {
    pub request: FetchThread,
    pub result: Result<(Vec<Post>, DateTime<Utc>), FetchError>,
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
        let last_modified = self.get_last_modified(&msg);
        let future = Box::new(
            fetch_with_last_modified(&msg, last_modified, &self.client, ctx.address())
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

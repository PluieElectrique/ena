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

    fn handle(&mut self, msg: UpdateLastModified, _: &mut Self::Context) -> Self::Result {
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
            self.thread_sender
                .clone()
                .send((msg, last_modified))
                .map(|_| ())
                .map_err(|err| error!("{}", err)),
        );
    }
}

pub struct FetchThreadList(pub Board);
impl Message for FetchThreadList {
    type Result = Result<(Vec<Thread>, DateTime<Utc>), FetchError>;
}

impl ToUri for &FetchThreadList {
    fn to_uri(&self) -> Uri {
        format!("{}/{}/threads.json", API_URI_PREFIX, self.0)
            .parse()
            .unwrap()
    }
}

impl Handler<FetchThreadList> for Fetcher {
    type Result = RateLimitedResponse<(Vec<Thread>, DateTime<Utc>), FetchError>;
    fn handle(&mut self, msg: FetchThreadList, ctx: &mut Self::Context) -> Self::Result {
        RateLimitedResponse {
            sender: self.thread_list_sender.clone(),
            future: fetch_thread_list(
                &msg,
                self.get_last_modified(&msg),
                &self.client,
                ctx.address(),
            ),
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
    fn handle(&mut self, msg: FetchArchive, _: &mut Self::Context) -> Self::Result {
        RateLimitedResponse {
            sender: self.thread_list_sender.clone(),
            future: fetch_archive(&msg, &self.client),
        }
    }
}

#[derive(Message)]
pub struct FetchMedia(pub Board, pub Vec<String>);

impl Handler<FetchMedia> for Fetcher {
    type Result = ();
    fn handle(&mut self, msg: FetchMedia, _: &mut Self::Context) {
        // If a media future panics, the media runtime will crash and the sender will close. The
        // Actix system has its own runtime, so it won't crash. But, we can't recover from a media
        // runtime panic, so if the media runtime crashes we crash the Actix system as well.
        if self.media_sender.is_closed() {
            panic!("Media sender is closed");
        }

        self.runtime.spawn(
            self.media_sender
                .clone()
                .send(msg)
                .map(|_| ())
                .map_err(|err| error!("{}", err)),
        );
    }
}

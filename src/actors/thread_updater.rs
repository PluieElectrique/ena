use std::collections::HashMap;
use std::hash::Hasher;

use actix::prelude::*;
use chrono::prelude::*;
use futures::future;
use futures::prelude::*;
use twox_hash::XxHash;

use super::board_poller::{BoardUpdate, ThreadUpdate};
use super::database::*;
use super::fetcher::*;
use four_chan::{self, Board, Post};

pub struct ThreadUpdater {
    board: Board,
    thread_meta: HashMap<u64, ThreadMetadata>,
    fetcher: Addr<Fetcher>,
    database: Addr<Database>,
    refetch_archived_threads: bool,
    always_add_archive_times: bool,
}

impl Actor for ThreadUpdater {
    type Context = Context<Self>;
}

impl ThreadUpdater {
    pub fn new(
        board: Board,
        database: Addr<Database>,
        fetcher: Addr<Fetcher>,
        refetch_archived_threads: bool,
        always_add_archive_times: bool,
    ) -> Self {
        Self {
            board,
            thread_meta: HashMap::new(),
            fetcher,
            database,
            refetch_archived_threads,
            always_add_archive_times,
        }
    }

    fn insert_posts(&mut self, posts: Vec<Post>) {
        if posts.is_empty() {
            return;
        }

        let board = self.board;
        let fetcher = self.fetcher.clone();
        Arbiter::spawn(
            // TODO: retry on error?
            self.database
                .send(InsertPosts(self.board, posts))
                .map_err(|err| log_error!(&err))
                .and_then(|res| res.map_err(|err| error!("{}", err)))
                .and_then(move |filenames| {
                    future::join_all(
                        filenames
                            .into_iter()
                            .map(move |filename| fetcher.send(FetchMedia(board, filename))),
                    ).map(|_| ())
                    .map_err(|err| error!("{}", err))
                }),
        );
    }

    fn handle_new(&mut self, no: u64, ctx: &mut <Self as Actor>::Context) {
        let future = self.fetcher
            .send(FetchThread(self.board, no))
            // TODO: Retry on error?
            .map_err(|err| log_error!(&err))
            .into_actor(self)
            .map(move |res, act, _ctx| {
                match res {
                    Ok((thread, _)) => {
                        debug!("Inserting new thread /{}/ No. {}", act.board, no);
                        act.thread_meta.insert(no, ThreadMetadata::from_thread(&thread));
                        act.insert_posts(thread);
                    }
                    Err(err) => match err {
                        FetchError::NotFound => {
                            warn!("/{}/ No. {} was deleted before it could be inserted",
                                act.board,
                                no,
                            );
                            act.thread_meta.remove(&no);
                            act.handle_removed(vec![(no, RemovedStatus::Deleted)], Utc::now());
                        },
                        // TODO: retry request
                        _ => log_error!(&err),
                    }
                }
            });
        ctx.spawn(future);
    }

    fn handle_modified(
        &mut self,
        no: u64,
    ) -> impl ActorFuture<Actor = Self, Item = (), Error = ()> {
        self.fetcher
            .send(FetchThread(self.board, no))
            // TODO: Retry on error?
            .map_err(|err| log_error!(&err))
            .into_actor(self)
            .map(move |res, act, _ctx| {
                match res {
                    Ok((mut thread, last_modified)) => {
                        let curr_meta = ThreadMetadata::from_thread(&thread);
                        let prev_meta = match act.thread_meta.remove(&no) {
                            Some(meta) => meta,
                            None => {
                                error!(
                                    "/{}/ No. {} was \"modified\" but not found in the threads map. Inserting whole thread",
                                    act.board,
                                    no,
                                );
                                act.thread_meta.insert(no, curr_meta);
                                act.insert_posts(thread);
                                return;
                            },
                        };

                        if prev_meta.op_data != curr_meta.op_data {
                            debug!("Updating OP data of /{}/ No. {}", act.board, no);
                            Arbiter::spawn(
                                act.database
                                    .send(UpdateOp(act.board, no, curr_meta.op_data.clone()))
                                    .map_err(|err| error!("{}", err))
                                    .and_then(|res| res.map_err(|err| error!("{}", err)))
                            );
                        }

                        let mut new_posts = vec![];
                        let mut modified_posts = vec![];
                        let mut deleted_posts = vec![];
                        {
                            let mut prev_iter = prev_meta.posts.iter();
                            let mut curr_iter = curr_meta.posts.iter().enumerate();

                            let mut curr_meta = curr_iter.next();

                            loop {
                                match (prev_iter.next(), curr_meta) {
                                    (Some(prev), Some((i, curr))) => {
                                        if prev.no == curr.no {
                                            if prev.metadata != curr.metadata {
                                                modified_posts.push((
                                                    thread[i].no,
                                                    thread[i].comment.take(),
                                                    thread[i].image.as_ref().map(|i| i.spoiler),
                                                ));
                                            }
                                            curr_meta = curr_iter.next();
                                        } else {
                                            deleted_posts.push((prev.no, RemovedStatus::Deleted));
                                        }
                                    }
                                    (Some(prev), None) => {
                                        deleted_posts.push((prev.no, RemovedStatus::Deleted));
                                    },
                                    (None, Some((i, _))) => {
                                        new_posts = thread.split_off(i);
                                        break;
                                    }
                                    (None, None) => break,
                                }
                            }
                        }
                        debug!(
                            "/{}/ No. {}: {:>2} new, {:>2} modified, {:>2} deleted",
                            act.board,
                            no,
                            new_posts.len(),
                            modified_posts.len(),
                            deleted_posts.len(),
                        );

                        act.insert_posts(new_posts);
                        if !modified_posts.is_empty() {
                            Arbiter::spawn(
                                act.database.send(UpdatePost(act.board, modified_posts))
                                    .map_err(|err| error!("{}", err))
                                    .and_then(|res| res.map_err(|err| error!("{}", err)))
                            );
                        }
                        act.handle_removed(deleted_posts, last_modified);
                        act.thread_meta.insert(no, curr_meta);
                    }
                    Err(err) => match err {
                        FetchError::NotModified => {}
                        FetchError::NotFound => {
                            warn!("/{}/ No. {} was deleted before it could be updated",
                                act.board,
                                no,
                            );
                            act.thread_meta.remove(&no);
                            act.handle_removed(vec![(no, RemovedStatus::Deleted)], Utc::now());
                        },
                        // TODO: retry request
                        _ => log_error!(&err),
                    }
                }
            })
    }

    fn handle_removed(&self, removed_posts: Vec<(u64, RemovedStatus)>, time: DateTime<Utc>) {
        if removed_posts.is_empty() {
            return;
        }

        let board = self.board;
        Arbiter::spawn(
            self.database
                .send(MarkPostsRemoved(self.board, removed_posts, time))
                .map_err(|err| error!("{}", err))
                .and_then(move |res| {
                    res.map_err(|err| {
                        error!("Failed to mark posts from /{}/ as removed: {}", board, err)
                    })
                }),
        );
    }
}

impl Handler<BoardUpdate> for ThreadUpdater {
    type Result = ();

    fn handle(&mut self, msg: BoardUpdate, ctx: &mut Self::Context) {
        let mut removed_threads = vec![];

        use self::ThreadUpdate::*;
        for thread in msg.0 {
            match thread {
                New(no) => self.handle_new(no, ctx),
                Modified(no) => {
                    ctx.spawn(self.handle_modified(no));
                }
                BumpedOff(no) => {
                    if self.board.is_archived() && self.refetch_archived_threads {
                        debug!("/{}/ No. {} was bumped off, refetching", self.board, no);
                        ctx.spawn(self.handle_modified(no).map(move |_, act, _ctx| {
                            act.thread_meta.remove(&no);
                        }));
                    } else {
                        debug!("/{}/ No. {} was bumped off", self.board, no);
                        if self.board.is_archived() || self.always_add_archive_times {
                            removed_threads.push((no, RemovedStatus::Archived));
                        }
                        self.thread_meta.remove(&no);
                    }
                }
                Deleted(no) => {
                    // If this thread isn't in the map, then we've already handled its deletion
                    if self.thread_meta.remove(&no).is_some() {
                        debug!("/{}/ No. {} was deleted", self.board, no);
                        removed_threads.push((no, RemovedStatus::Deleted));
                    }
                }
            }
        }
        self.handle_removed(removed_threads, msg.1);
    }
}

struct ThreadMetadata {
    op_data: four_chan::OpData,
    posts: Vec<PostMetadata>,
}

impl ThreadMetadata {
    fn from_thread(thread: &[four_chan::Post]) -> Self {
        Self {
            op_data: thread[0].op_data.clone(),
            posts: thread.iter().map(PostMetadata::from).collect(),
        }
    }
}

/// Used to determine if a post was modified or not
struct PostMetadata {
    no: u64,
    /// Hash of a comment before HTML cleaning and the image spoiler flag
    metadata: (Option<u64>, Option<bool>),
}

impl<'a> From<&'a Post> for PostMetadata {
    fn from(post: &Post) -> Self {
        let comment_hash = post.comment.as_ref().map(|c| {
            let mut hasher = XxHash::default();
            hasher.write(c.as_bytes());
            hasher.finish()
        });
        let spoiler = post.image.as_ref().map(|i| i.spoiler);

        Self {
            no: post.no,
            metadata: (comment_hash, spoiler),
        }
    }
}

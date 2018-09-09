use std::collections::HashMap;
use std::hash::Hasher;

use actix::prelude::*;
use chrono::prelude::*;
use futures::prelude::*;
use log::Level;
use twox_hash::XxHash;

use super::board_poller::*;
use super::database::*;
use super::fetcher::*;
use four_chan::{self, Board, OpData, Post};

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
        if !posts.is_empty() {
            let board = self.board;
            let fetcher = self.fetcher.clone();
            Arbiter::spawn(
                self.database
                    .send(InsertPosts(self.board, posts))
                    .map_err(|err| log_error!(&err))
                    .and_then(|res| res.map_err(|err| error!("{}", err)))
                    .and_then(move |filenames| {
                        fetcher
                            .send(FetchMedia(board, filenames))
                            .map_err(|err| error!("{}", err))
                    }),
            );
        }
    }

    fn modify_posts(&self, modified_posts: Vec<(u64, Option<String>, Option<bool>)>) {
        if !modified_posts.is_empty() {
            Arbiter::spawn(
                self.database
                    .send(UpdatePost(self.board, modified_posts))
                    .map_err(|err| error!("{}", err))
                    .and_then(|res| res.map_err(|err| error!("{}", err))),
            );
        }
    }

    fn update_op_data(&self, no: u64, op_data: OpData) {
        Arbiter::spawn(
            self.database
                .send(UpdateOp(self.board, no, op_data))
                .map_err(|err| error!("{}", err))
                .and_then(|res| res.map_err(|err| error!("{}", err))),
        );
    }

    fn remove_posts(&self, removed_posts: Vec<(u64, RemovedStatus)>, time: DateTime<Utc>) {
        if !removed_posts.is_empty() {
            Arbiter::spawn(
                self.database
                    .send(MarkPostsRemoved(self.board, removed_posts, time))
                    .map_err(|err| error!("{}", err))
                    .and_then(|res| res.map_err(|err| error!("{}", err))),
            );
        }
    }

    fn process_modified(
        &mut self,
        no: u64,
        (mut thread, last_modified): (Vec<Post>, DateTime<Utc>),
        curr_meta: &ThreadMetadata,
        prev_meta: &ThreadMetadata,
    ) {
        if curr_meta.op_data != prev_meta.op_data {
            debug!("/{}/ No. {}: Updating OP data", self.board, no);
            self.update_op_data(no, curr_meta.op_data.clone());
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
                    }
                    (None, Some((i, _))) => {
                        new_posts = thread.split_off(i);
                        break;
                    }
                    (None, None) => break,
                }
            }
        }
        if log_enabled!(Level::Debug) {
            let n = new_posts.len();
            let m = modified_posts.len();
            let d = deleted_posts.len();
            // Nice formatting by horrendously using arithmetic as logic
            if (n + m + d) > 0 {
                debug!(
                    "/{}/ No. {}: {}{}{}{}{}",
                    self.board,
                    no,
                    zero_format!("{} new", n),
                    if n * (m + d) == 0 { "" } else { ", " },
                    zero_format!("{} modified", m),
                    if d * m == 0 { "" } else { ", " },
                    zero_format!("{} deleted", d),
                );
            }
        }

        self.insert_posts(new_posts);
        self.modify_posts(modified_posts);
        self.remove_posts(deleted_posts, last_modified);
    }

    fn process_thread(&self, no: u64, ctx: &mut <Self as Actor>::Context, handle_deleted: bool) {
        let future = self
            .fetcher
            .send(FetchThread(self.board, no))
            .map_err(|err| log_error!(&err))
            .into_actor(self)
            .map(move |res, act, _ctx| match res {
                Ok((thread, last_modified)) => {
                    let curr_meta = ThreadMetadata::from_thread(&thread, last_modified);

                    if let Some(prev_meta) = act.thread_meta.remove(&no) {
                        if last_modified < prev_meta.last_modified {
                            error!(
                                "/{}/ No. {} Ignoring old thread data: {} < {}",
                                act.board, no, last_modified, prev_meta.last_modified
                            );
                        } else {
                            act.process_modified(
                                no,
                                (thread, last_modified),
                                &curr_meta,
                                &prev_meta,
                            );
                        }
                    } else {
                        debug!("/{}/ No. {}: Inserting new thread", act.board, no);
                        act.insert_posts(thread);
                    }

                    if !curr_meta.op_data.archived {
                        act.thread_meta.insert(no, curr_meta);
                    }
                }
                Err(err) => match err {
                    FetchError::NotModified => {}
                    FetchError::NotFound => {
                        warn!(
                            "/{}/ No. {} was deleted before it could be processed",
                            act.board, no,
                        );
                        if handle_deleted {
                            act.thread_meta.remove(&no);
                            act.remove_posts(vec![(no, RemovedStatus::Deleted)], Utc::now());
                        }
                    }
                    _ => log_error!(&err),
                },
            });
        ctx.spawn(future);
    }
}

impl Handler<BoardUpdate> for ThreadUpdater {
    type Result = ();

    fn handle(&mut self, msg: BoardUpdate, ctx: &mut Self::Context) {
        let mut removed_threads = vec![];

        use self::ThreadUpdate::*;
        for thread in msg.0 {
            match thread {
                New(no) | Modified(no) => self.process_thread(no, ctx, true),
                BumpedOff(no) => {
                    // If this thread isn't in the map, it's already been archived or deleted
                    if self.thread_meta.contains_key(&no) {
                        if self.board.is_archived() && self.refetch_archived_threads {
                            debug!("/{}/ No. {}: Bumped off, refetching", self.board, no);
                            self.process_thread(no, ctx, true);
                        } else {
                            debug!("/{}/ No. {}: Bumped off", self.board, no);
                            if self.board.is_archived() || self.always_add_archive_times {
                                removed_threads.push((no, RemovedStatus::Archived));
                            }
                            self.thread_meta.remove(&no);
                        }
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
        self.remove_posts(removed_threads, msg.1);
    }
}

impl Handler<ArchiveUpdate> for ThreadUpdater {
    type Result = ();

    fn handle(&mut self, msg: ArchiveUpdate, ctx: &mut Self::Context) {
        ctx.spawn(
            self.database
                .send(GetUnarchivedThreads(self.board, msg.0))
                .into_actor(self)
                .map(|res, act, ctx| {
                    match res {
                        Ok(threads) => for no in threads {
                            // We pass false for handle_deleted because if an archived thread 404's,
                            // then it expired before we processed it, and was not deleted.
                            act.process_thread(no, ctx, false);
                        },
                        Err(err) => error!(
                            "/{}/: Failed to process archived threads: {}",
                            act.board, err
                        ),
                    }
                }).map_err(|err, act, _ctx| {
                    error!(
                        "/{}/: Failed to process archived threads: {}",
                        act.board, err
                    )
                }),
        );
    }
}

struct ThreadMetadata {
    last_modified: DateTime<Utc>,
    op_data: OpData,
    posts: Vec<PostMetadata>,
}

impl ThreadMetadata {
    fn from_thread(thread: &[four_chan::Post], last_modified: DateTime<Utc>) -> Self {
        Self {
            last_modified,
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

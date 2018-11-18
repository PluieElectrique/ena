use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::Arc;

use actix::prelude::*;
use chrono::prelude::*;
use futures::future::{self, Either};
use futures::prelude::*;
use log::Level;
use twox_hash::XxHash;

use super::board_poller::*;
use super::database::*;
use super::fetcher::*;
use config::Config;
use four_chan::{self, Board, OpData, Post};

/// An actor which updates threads when it receives change notifications from
/// [`BoardPoller`](struct.BoardPoller.html).
pub struct ThreadUpdater {
    thread_meta: HashMap<(Board, u64), ThreadMetadata>,
    fetcher: Arc<Addr<Fetcher>>,
    database: Addr<Database>,
    refetch_archived_threads: bool,
    always_add_archive_times: bool,
}

impl Actor for ThreadUpdater {
    type Context = Context<Self>;
}

impl ThreadUpdater {
    pub fn new(config: &Config, database: Addr<Database>, fetcher: Addr<Fetcher>) -> Self {
        Self {
            thread_meta: HashMap::new(),
            fetcher: Arc::new(fetcher),
            database,
            refetch_archived_threads: config.asagi_compat.refetch_archived_threads,
            always_add_archive_times: config.asagi_compat.always_add_archive_times,
        }
    }

    fn insert_posts(&mut self, board: Board, no: u64, posts: Vec<Post>) {
        if !posts.is_empty() {
            let fetcher = self.fetcher.clone();
            Arbiter::spawn(
                self.database
                    .send(InsertPosts(board, no, posts))
                    .map_err(|err| log_error!(&err))
                    .and_then(|res| res.map_err(|err| error!("{}", err)))
                    .and_then(move |filenames| {
                        if filenames.is_empty() {
                            Either::A(future::ok(()))
                        } else {
                            Either::B(
                                fetcher
                                    .send(FetchMedia(board, filenames))
                                    .map_err(|err| error!("{}", err)),
                            )
                        }
                    }),
            );
        }
    }

    fn modify_posts(&self, board: Board, modified_posts: Vec<(u64, Option<String>, Option<bool>)>) {
        if !modified_posts.is_empty() {
            Arbiter::spawn(
                self.database
                    .send(UpdatePost(board, modified_posts))
                    .map_err(|err| error!("{}", err))
                    .and_then(|res| res.map_err(|err| error!("{}", err))),
            );
        }
    }

    fn update_op_data(&self, board: Board, no: u64, op_data: OpData) {
        Arbiter::spawn(
            self.database
                .send(UpdateOp(board, no, op_data))
                .map_err(|err| error!("{}", err))
                .and_then(|res| res.map_err(|err| error!("{}", err))),
        );
    }

    fn remove_posts(
        &self,
        board: Board,
        removed_posts: Vec<(u64, RemovedStatus)>,
        time: DateTime<Utc>,
    ) {
        if !removed_posts.is_empty() {
            Arbiter::spawn(
                self.database
                    .send(MarkPostsRemoved(board, removed_posts, time))
                    .map_err(|err| error!("{}", err))
                    .and_then(|res| res.map_err(|err| error!("{}", err))),
            );
        }
    }

    fn process_modified(
        &mut self,
        board: Board,
        no: u64,
        mut thread: Vec<Post>,
        last_modified: DateTime<Utc>,
        curr_meta: &ThreadMetadata,
        prev_meta: &ThreadMetadata,
    ) {
        if curr_meta.op_data != prev_meta.op_data {
            debug!("/{}/ No. {}: Updating OP data", board, no);
            self.update_op_data(board, no, curr_meta.op_data.clone());
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
                    board,
                    no,
                    zero_format!("{} new", n),
                    if n * (m + d) == 0 { "" } else { ", " },
                    zero_format!("{} modified", m),
                    if d * m == 0 { "" } else { ", " },
                    zero_format!("{} deleted", d),
                );
            }
        }

        self.insert_posts(board, no, new_posts);
        self.modify_posts(board, modified_posts);
        self.remove_posts(board, deleted_posts, last_modified);
    }

    fn process_thread(&mut self, msg: FetchedThread) {
        let FetchedThread { request, result } = msg;
        let FetchThread(board, no, from_archive_json) = request;

        match result {
            Ok((mut thread, last_modified)) => {
                // Sort ascending by no. The posts should already be sorted, but I have seen one
                // case where they weren't. So it's better to be safe.
                thread.sort_by(|a, b| a.no.cmp(&b.no));

                let curr_meta = ThreadMetadata::from_thread(&thread);
                if let Some(prev_meta) = self.thread_meta.remove(&(board, no)) {
                    self.process_modified(board, no, thread, last_modified, &curr_meta, &prev_meta);
                } else {
                    debug!("/{}/ No. {}: Inserting thread", board, no);
                    self.insert_posts(board, no, thread);
                }

                if !curr_meta.op_data.archived {
                    self.thread_meta.insert((board, no), curr_meta);
                }
            }
            Err(err) => match err {
                FetchError::NotModified => {}
                FetchError::NotFound(_) => {
                    if from_archive_json {
                        // If a thread loaded from archive.json 404's, then it expired before we
                        // could process it, and was not deleted. So, we don't mark it as such.
                        warn!(
                            "/{}/ No. {}: Archived thread expired before it could be processed",
                            board, no,
                        );
                    } else {
                        warn!(
                            "/{}/ No. {}: Thread deleted before it could be processed",
                            board, no,
                        );
                        self.thread_meta.remove(&(board, no));
                        self.remove_posts(board, vec![(no, RemovedStatus::Deleted)], Utc::now());
                    }
                }
                _ => error!("/{}/ No. {} fetch failed: {}", board, no, err),
            },
        }
    }
}

impl Handler<FetchedThread> for ThreadUpdater {
    type Result = ();

    fn handle(&mut self, msg: FetchedThread, _: &mut Self::Context) {
        self.process_thread(msg);
    }
}

impl Handler<BoardUpdate> for ThreadUpdater {
    type Result = ();

    fn handle(&mut self, msg: BoardUpdate, _: &mut Self::Context) {
        let mut threads_to_fetch = vec![];
        let mut removed_threads = vec![];
        let BoardUpdate(board, updates, last_modified) = msg;

        for thread in updates {
            use self::ThreadUpdate::*;
            match thread {
                New(no) | Modified(no) => threads_to_fetch.push(no),
                BumpedOff(no) => {
                    // If this thread isn't in the map, it's already been archived or deleted
                    if self.thread_meta.contains_key(&(board, no)) {
                        if board.is_archived() && self.refetch_archived_threads {
                            debug!("/{}/ No. {}: Bumped off, refetching", board, no);
                            threads_to_fetch.push(no);
                        } else {
                            debug!("/{}/ No. {}: Bumped off", board, no);
                            if board.is_archived() || self.always_add_archive_times {
                                removed_threads.push((no, RemovedStatus::Archived));
                            }
                            self.thread_meta.remove(&(board, no));
                        }
                    }
                }
                Deleted(no) => {
                    // If this thread isn't in the map, then we've already handled its deletion
                    if self.thread_meta.remove(&(board, no)).is_some() {
                        debug!("/{}/ No. {} was deleted", board, no);
                        removed_threads.push((no, RemovedStatus::Deleted));
                    }
                }
            }
        }
        self.remove_posts(board, removed_threads, last_modified);
        if !threads_to_fetch.is_empty() {
            Arbiter::spawn(
                self.fetcher
                    .send(FetchThreads(board, threads_to_fetch, false))
                    .map_err(|err| log_error!(&err)),
            );
        }
    }
}

impl Handler<ArchiveUpdate> for ThreadUpdater {
    type Result = ();

    fn handle(&mut self, msg: ArchiveUpdate, ctx: &mut Self::Context) {
        let ArchiveUpdate(board, nums) = msg;
        ctx.spawn(
            self.database
                .send(GetUnarchivedThreads(board, nums))
                .into_actor(self)
                .map(move |res, act, _| match res {
                    Ok(threads) => {
                        let len = threads.len();
                        debug!(
                            "/{}/: Found {} new archived thread{}",
                            board,
                            len,
                            if len == 1 { "" } else { "s" },
                        );
                        if !threads.is_empty() {
                            Arbiter::spawn(
                                act.fetcher
                                    .send(FetchThreads(board, threads, true))
                                    .map_err(|err| log_error!(&err)),
                            );
                        }
                    }
                    Err(err) => error!("/{}/: Failed to process archived threads: {}", board, err),
                }).map_err(move |err, _act, _ctx| {
                    error!("/{}/: Failed to process archived threads: {}", board, err)
                }),
        );
    }
}

struct ThreadMetadata {
    op_data: OpData,
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

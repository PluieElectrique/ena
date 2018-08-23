use std::collections::HashMap;

use actix::prelude::*;
use futures::future;
use futures::prelude::*;

use super::board_poller::{BoardUpdate, ThreadUpdate};
use super::database::*;
use four_chan::fetcher::*;
use four_chan::{self, Board};

pub struct ThreadUpdater {
    board: Board,
    threads: HashMap<u64, Thread>,
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
            threads: HashMap::new(),
            fetcher,
            database,
            refetch_archived_threads,
            always_add_archive_times,
        }
    }

    fn insert_new_thread(&mut self, no: u64, ctx: &mut <Self as Actor>::Context) {
        let future = self
            .fetcher
            .send(FetchThread(self.board, no))
            // TODO: Retry on error?
            .map_err(|err| log_error!(&err))
            .into_actor(self)
            .map(move |res, act, _ctx| {
                match res {
                    Ok(thread) => {
                        act.threads.insert(no, Thread::from_thread(&thread));

                        let board = act.board;
                        let fetcher = act.fetcher.clone();
                        Arbiter::spawn(
                            // TODO: retry on error?
                            act.database
                                .send(InsertNewThread(act.board, thread))
                                .map_err(|err| log_error!(&err))
                                .and_then(|res| res.map_err(|err| error!("{}", err)))
                                .and_then(move |filenames| {
                                    future::join_all(filenames.into_iter().map(move |filename| {
                                        fetcher.send(FetchMedia(board, filename))
                                    }))
                                    .map(|_| ())
                                    .map_err(|err| error!("{}", err))
                                })
                        );
                    }
                    Err(err) => match err {
                        FetchError::NotFound => warn!(
                            "404 Not Found for /{}/ No. {}. Thread deleted between threads.json poll and fetch?",
                            act.board,
                            no,
                        ),
                        // TODO: retry request
                        _ => log_error!(&err),
                    }
                }
            });
        ctx.spawn(future);
    }
}

impl Handler<BoardUpdate> for ThreadUpdater {
    type Result = ();

    fn handle(&mut self, msg: BoardUpdate, ctx: &mut Self::Context) {
        let mut removed_posts = vec![];

        use self::ThreadUpdate::*;
        for thread in msg.0 {
            match thread {
                New(no) => self.insert_new_thread(no, ctx),
                Modified(_no) => {
                    // TODO: Insert if op modified
                    // TODO: Find modified posts (banned) and insert
                    // TODO: Find deleted media and mark
                    // TODO: Insert new posts
                }
                BumpedOff(no) => {
                    self.threads.remove(&no);
                    if self.board.is_archived() {
                        if self.refetch_archived_threads {
                            // TODO: handle as modified
                        } else {
                            removed_posts.push((no, RemovedStatus::Archived));
                        }
                    } else if self.always_add_archive_times {
                        removed_posts.push((no, RemovedStatus::Archived));
                    }
                }
                Deleted(no) => {
                    self.threads.remove(&no);
                    removed_posts.push((no, RemovedStatus::Deleted));
                }
            }
        }

        if !removed_posts.is_empty() {
            let board = self.board;
            Arbiter::spawn(
                self.database
                    .send(MarkPostsRemoved(self.board, removed_posts, msg.1))
                    .map_err(|err| error!("{}", err))
                    .and_then(move |res| {
                        res.map_err(|err| {
                            error!("Failed to mark posts from /{}/ as removed: {}", board, err)
                        })
                    }),
            );
        }
    }
}

struct Thread {
    no: u64,
    op_data: four_chan::OpData,
    posts: Vec<PostMetadata>,
}

impl Thread {
    fn from_thread(thread: &[four_chan::Post]) -> Self {
        let posts = thread
            .iter()
            .map(|post| PostMetadata {
                no: post.no,
                comment_len: post.comment.as_ref().map(|c| c.len()).unwrap_or(0),
                //file_deleted: post.image.as_ref().map(|i| i.file_deleted).unwrap_or(false),
            }).collect();

        Self {
            no: thread[0].no,
            op_data: thread[0].op_data.clone(),
            posts,
        }
    }
}

/// Used to determine if a post was modified or not
struct PostMetadata {
    no: u64,
    /// Length of a comment before HTML cleaning. Used to detect if "(USER WAS BANNED FOR THIS
    /// POST)" was added to the comment.
    comment_len: usize,
    // The Asagi/FoolFuuka schema currently doesn't track this
    //file_deleted: bool,
}

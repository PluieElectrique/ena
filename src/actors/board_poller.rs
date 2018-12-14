use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use ::actix::{fut, prelude::*};
use chrono::prelude::*;
use futures::prelude::*;
use log::Level;
use tokio::timer::Delay;

use super::{fetcher::*, ThreadUpdater};
use crate::{
    config::{Config, ScrapingConfig},
    four_chan::{Board, Thread},
};

#[derive(Message)]
pub struct ArchiveUpdate(pub Board, pub Vec<u64>);

#[derive(Message)]
pub struct BoardUpdate(pub Board, pub Vec<ThreadUpdate>, pub DateTime<Utc>);

pub enum ThreadUpdate {
    New(u64),
    Modified(u64),
    BumpedOff(u64),
    Deleted(u64),
}

/// An actor which watches a board's threads and sends updates to
/// [`ThreadUpdater`](struct.ThreadUpdater.html).
pub struct BoardPoller {
    boards: Arc<HashMap<Board, ScrapingConfig>>,
    threads: HashMap<Board, Vec<Thread>>,
    thread_updater: Arc<Addr<ThreadUpdater>>,
    fetcher: Addr<Fetcher>,
}

impl Actor for BoardPoller {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        for (&board, config) in self.boards.iter() {
            if config.fetch_archive && board.is_archived() {
                self.poll_archive(board, ctx);
            }
            self.poll(board, ctx);
        }
    }
}

impl BoardPoller {
    pub fn new(
        config: &Config,
        thread_updater: Addr<ThreadUpdater>,
        fetcher: Addr<Fetcher>,
    ) -> Self {
        let mut threads = HashMap::new();
        for &board in config.boards.keys() {
            threads.insert(board, vec![]);
        }
        threads.shrink_to_fit();

        Self {
            boards: config.boards.clone(),
            threads,
            thread_updater: Arc::new(thread_updater),
            fetcher,
        }
    }

    fn update_threads(
        &mut self,
        board: Board,
        mut curr_threads: Vec<Thread>,
        last_modified: DateTime<Utc>,
    ) {
        use self::ThreadUpdate::*;
        let mut updates = vec![];

        let push_removed = {
            let anchor_index = curr_threads.last().map(|last_thread| {
                self.threads[&board]
                    .iter()
                    .rev()
                    .find(|thread| thread.no == last_thread.no)
                    // If a board is fast and our poll interval is long, it might be possible for a
                    // thread to be bumped and then fall to the bottom of the board in one interval.
                    // This would break the heuristic and could cause bumped-off threads to be
                    // marked as deleted. So, we check last_modified to ensure that this thread
                    // hasn't been bumped. Note that this check will also reject saged threads,
                    // which are valid anchors. That's okay, though, because we would rather reject
                    // some valid anchors than accept invalid ones.
                    .filter(|thread| thread.last_modified == last_thread.last_modified)
                    .map(|thread| thread.bump_index)
            });

            move |thread: &Thread, updates: &mut Vec<ThreadUpdate>| {
                match anchor_index {
                    Some(Some(anchor)) => {
                        // We found an anchor and will use it to determine if a thread has been
                        // deleted or bumped off.
                        if thread.bump_index < anchor {
                            updates.push(Deleted(thread.no));
                        } else {
                            updates.push(BumpedOff(thread.no));
                        }
                    }
                    Some(None) => {
                        // The board isn't empty, but we didn't find a valid anchor. Without enough
                        // information, we assume all removed threads were bumped off.
                        updates.push(BumpedOff(thread.no));
                    }
                    None => {
                        // The board is empty, so all threads must have been deleted.
                        updates.push(Deleted(thread.no));
                    }
                }
            }
        };

        // Sort ascending by no
        curr_threads.sort_by(|a, b| a.no.cmp(&b.no));

        let mut prev_iter = self.threads[&board].iter();
        let mut curr_iter = curr_threads.iter();

        let mut curr_thread = curr_iter.next();

        loop {
            match (prev_iter.next(), curr_thread) {
                (Some(prev), Some(curr)) => {
                    assert!(prev.no <= curr.no);

                    if prev.no == curr.no {
                        assert!(prev.last_modified <= curr.last_modified);

                        if prev.last_modified < curr.last_modified {
                            updates.push(Modified(curr.no));
                        }
                        curr_thread = curr_iter.next();
                    } else if prev.no < curr.no {
                        push_removed(prev, &mut updates);
                    }
                }
                (Some(prev), None) => {
                    push_removed(prev, &mut updates);
                }
                (None, Some(curr)) => {
                    updates.push(New(curr.no));
                    curr_thread = curr_iter.next();
                }
                (None, None) => break,
            }
        }

        if log_enabled!(Level::Debug) {
            let mut new = 0;
            let mut modified = 0;
            let mut bumped_off = 0;
            let mut deleted = 0;

            for update in &updates {
                match update {
                    New(_) => new += 1,
                    Modified(_) => modified += 1,
                    BumpedOff(_) => bumped_off += 1,
                    Deleted(_) => deleted += 1,
                }
            }

            let len = updates.len();
            debug!(
                "/{}/: Updating {} thread{}{}{}{}{}",
                board,
                len,
                if len == 1 { "" } else { "s" },
                zero_format!(", {} new", new),
                zero_format!(", {} modified", modified),
                zero_format!(", {} bumped off", bumped_off),
                zero_format!(", {} deleted", deleted),
            );
        }

        let thread_updater = self.thread_updater.clone();
        Arbiter::spawn(
            // It often takes 1-2 seconds for new data to go from an updated last_modified in
            // threads.json to actually showing up at the .json endpoint. We wait 3 seconds to be
            // safe and ensure that ThreadUpdater doesn't read old data.
            Delay::new(Instant::now() + Duration::from_secs(3))
                .map_err(|err| error!("{}", err))
                .and_then(move |_| {
                    thread_updater
                        .send(BoardUpdate(board, updates, last_modified))
                        .map_err(|err| error!("{}", err))
                }),
        );
        self.threads.insert(board, curr_threads);
    }

    fn poll(&self, board: Board, ctx: &mut Context<Self>) {
        ctx.spawn(
            self.fetcher
                .send(FetchThreadList(board))
                .map_err(|err| log_error!(&err))
                .into_actor(self)
                .timeout(self.boards[&board].poll_interval, ())
                .then(move |res, act, ctx| {
                    if let Ok(res) = res {
                        match res {
                            Ok((threads, last_modified)) => {
                                act.update_threads(board, threads, last_modified);
                            }
                            Err(err) => match err {
                                FetchError::NotModified => {}
                                _ => error!("/{}/: Failed to fetch threads: {}", board, err),
                            },
                        }
                    }
                    ctx.run_later(act.boards[&board].poll_interval, move |act, ctx| {
                        act.poll(board, ctx);
                    });
                    fut::ok(())
                }),
        );
    }

    fn poll_archive(&self, board: Board, ctx: &mut Context<Self>) {
        ctx.spawn(
            self.fetcher
                .send(FetchArchive(board))
                .map_err(|err| log_error!(&err))
                .into_actor(self)
                .map(move |res, act, _ctx| match res {
                    Ok(threads) => {
                        let len = threads.len();
                        debug!(
                            "/{}/: Fetched {} archived thread{}",
                            board,
                            len,
                            if len == 1 { "" } else { "s" },
                        );
                        if !threads.is_empty() {
                            Arbiter::spawn(
                                act.thread_updater
                                    .send(ArchiveUpdate(board, threads))
                                    .map_err(|err| error!("{}", err)),
                            );
                        }
                    }
                    Err(err) => error!("/{}/: Failed to fetch archive: {}", board, err),
                })
                .map_err(move |err, _act, _ctx| {
                    error!("/{}/: Failed to fetch archive: {:?}", board, err)
                }),
        );
    }
}

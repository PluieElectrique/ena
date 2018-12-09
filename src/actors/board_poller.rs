use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::fut;
use actix::prelude::*;
use chrono::prelude::*;
use futures::prelude::*;
use log::Level;
use tokio::timer::Delay;

use super::fetcher::*;
use super::ThreadUpdater;
use config::Config;
use four_chan::{Board, Thread};

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
    boards: Vec<Board>,
    threads: HashMap<Board, Vec<Thread>>,
    interval: Duration,
    fetch_archive: bool,
    thread_updater: Arc<Addr<ThreadUpdater>>,
    fetcher: Addr<Fetcher>,
}

impl Actor for BoardPoller {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        for &board in &self.boards {
            if self.fetch_archive && board.is_archived() {
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
        let boards = config.scraping.boards.clone();
        let mut threads = HashMap::new();
        for &board in &boards {
            threads.insert(board, vec![]);
        }
        threads.shrink_to_fit();

        Self {
            boards,
            threads,
            interval: config.scraping.poll_interval,
            fetch_archive: config.scraping.fetch_archive,
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

        let push_removed: Box<Fn(&Thread, &mut Vec<_>)> = if curr_threads.is_empty() {
            // If the board is completely empty, all threads must have been deleted
            Box::new(|thread, updates| {
                updates.push(Deleted(thread.no));
            })
        } else {
            let last_thread = &curr_threads[curr_threads.len() - 1];
            let anchor_index = self.threads[&board]
                .iter()
                .rev()
                .find(|thread| thread.no == last_thread.no)
                // If a board is fast and our poll interval is long, a bumped thread might have
                // enough time to fall to the bottom of the board. This could cause bumped-off
                // threads to be marked as deleted. So, we check that last_modified hasn't changed.
                // Sage posts will also trigger this check, but that's okay, because we'd rather
                // reject good anchors than accept incorrect ones.
                .filter(|thread| thread.last_modified == last_thread.last_modified)
                .map(|thread| thread.bump_index);

            Box::new(move |thread, updates| {
                match anchor_index {
                    Some(anchor) => {
                        if thread.bump_index < anchor {
                            updates.push(Deleted(thread.no));
                        } else {
                            updates.push(BumpedOff(thread.no));
                        }
                    }
                    None => {
                        // If all of the threads have changed, we have no information and can't
                        // assume that any thread was deleted
                        updates.push(BumpedOff(thread.no));
                    }
                }
            })
        };

        // Sort ascending by no
        curr_threads.sort_by(|a, b| a.no.cmp(&b.no));

        // TODO: Remove scope when NLL stabilizes
        {
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
                .timeout(self.interval, ())
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
                    ctx.run_later(act.interval, move |act, ctx| {
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

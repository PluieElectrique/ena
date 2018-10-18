use std::time::{Duration, Instant};

use actix::fut;
use actix::prelude::*;
use chrono::prelude::*;
use futures::prelude::*;
use log::Level;
use tokio::timer::Delay;

use super::fetcher::*;
use super::ThreadUpdater;
use four_chan::{Board, Thread};

#[derive(Message)]
pub struct ArchiveUpdate(pub Board, pub Vec<u64>);

#[derive(Message)]
pub struct BoardUpdate(pub Board, pub Vec<ThreadUpdate>, pub DateTime<Utc>);

#[derive(Debug)]
pub enum ThreadUpdate {
    New(u64),
    Modified(u64),
    BumpedOff(u64),
    Deleted(u64),
}

/// An actor which watches a board's threads and sends updates to its
/// [`ThreadUpdater`](struct.ThreadUpdater.html).
pub struct BoardPoller {
    board: Board,
    threads: Vec<Thread>,
    interval: Duration,
    thread_updater: Addr<ThreadUpdater>,
    fetcher: Addr<Fetcher>,
}

impl Actor for BoardPoller {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        if self.board.is_archived() {
            self.poll_archive(ctx);
        }
        self.poll(ctx);
    }
}

impl BoardPoller {
    pub fn new(
        board: Board,
        interval: Duration,
        thread_updater: Addr<ThreadUpdater>,
        fetcher: Addr<Fetcher>,
    ) -> Self {
        Self {
            board,
            threads: vec![],
            interval,
            thread_updater,
            fetcher,
        }
    }

    fn update_threads(&mut self, mut curr_threads: Vec<Thread>, last_modified: DateTime<Utc>) {
        use self::ThreadUpdate::*;
        let mut updates = vec![];

        let push_removed = {
            let last_no = curr_threads[curr_threads.len() - 1].no;
            let anchor_index = self
                .threads
                .iter()
                .rev()
                // We don't check that last_modified hasn't changed. This means there is a slight
                // chance that the heuristic will fail when there is enough time between polls for
                // the anchor to be bumped to the top and fall back to the bottom. But, boards
                // usually aren't that fast and our poll interval is small, so this is unlikely.
                .find(|thread| thread.no == last_no)
                .map(|thread| thread.bump_index);

            move |thread: &Thread, updates: &mut Vec<_>| {
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
            }
        };

        // Sort ascending by no
        curr_threads.sort_by(|a, b| a.no.cmp(&b.no));

        {
            let mut prev_iter = self.threads.iter();
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
                self.board,
                len,
                if len == 1 { "" } else { "s" },
                zero_format!(", {} new", new),
                zero_format!(", {} modified", modified),
                zero_format!(", {} bumped off", bumped_off),
                zero_format!(", {} deleted", deleted),
            );
        }

        let future = self
            .thread_updater
            .send(BoardUpdate(self.board, updates, last_modified))
            .map_err(|err| error!("{}", err));
        Arbiter::spawn(
            // It often takes 1-2 seconds for new data to go from an updated last_modified in
            // threads.json to actually showing up at the .json endpoint. We wait 3 seconds to be
            // safe and ensure that ThreadUpdater doesn't read old data.
            Delay::new(Instant::now() + Duration::from_secs(3))
                .map_err(|err| error!("{}", err))
                .and_then(|_| future),
        );
        self.threads = curr_threads;
    }

    fn poll(&self, ctx: &mut Context<Self>) {
        ctx.spawn(
            self.fetcher
                .send(FetchThreads(self.board))
                .map_err(|err| log_error!(&err))
                .into_actor(self)
                .timeout(self.interval, ())
                .then(|res, act, ctx| {
                    if let Ok(res) = res {
                        match res {
                            Ok((threads, last_modified)) => {
                                act.update_threads(threads, last_modified);
                            }
                            Err(err) => match err {
                                FetchError::NotModified => {}
                                _ => error!("/{}/: Failed to fetch threads: {}", act.board, err),
                            },
                        }
                    }
                    ctx.run_later(act.interval, |act, ctx| {
                        act.poll(ctx);
                    });
                    fut::ok(())
                }),
        );
    }

    fn poll_archive(&self, ctx: &mut Context<Self>) {
        ctx.spawn(
            self.fetcher
                .send(FetchArchive(self.board))
                .map_err(|err| log_error!(&err))
                .into_actor(self)
                .map(|res, act, _ctx| match res {
                    Ok(threads) => {
                        let len = threads.len();
                        debug!(
                            "/{}/: Fetched {} archived thread{}",
                            act.board,
                            len,
                            if len == 1 { "" } else { "s" },
                        );
                        Arbiter::spawn(
                            act.thread_updater
                                .send(ArchiveUpdate(act.board, threads))
                                .map_err(|err| error!("{}", err)),
                        );
                    }
                    Err(err) => error!("/{}/: Failed to fetch archive: {}", act.board, err),
                }).map_err(|err, act, _ctx| {
                    error!("/{}/: Failed to fetch archive: {:?}", act.board, err)
                }),
        );
    }
}

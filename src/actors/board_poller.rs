use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use actix::{fut, prelude::*};
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
            // To determine if a removed thread was bumped off or deleted, we use the "anchor
            // thread" heuristic. Note that when in doubt, we always assume that a removed thread
            // was bumped off, as deletions are much rarer.
            //
            // Now, the anchor thread is the thread which:
            // 1. Is the last thread of the current thread list,
            // Note: Any current thread which satisfies 2 and 3 is an anchor, but the last thread
            //       makes for the best anchor, as it lets us catch as many deleted threads as
            //       possible. To keep things simple, if the last thread turns out to not be a valid
            //       anchor (which is unlikely), we don't consider other threads.
            let anchor_index = curr_threads.last().map(|last_thread| {
                self.threads[&board]
                    .iter()
                    .rev()
                    // 2. Appears in the previous thread list (i.e. is not a new thread), and
                    .find(|thread| thread.no == last_thread.no)
                    // 3. Has not been bumped since the last poll.
                    // Note: last_modified is updated when a new post is added and bumps the thread.
                    //       But it's also updated when the thread is saged. We should accept saged
                    //       threads as anchors, since they weren't bumped, but we can't tell them
                    //       apart from bumped threads. So, this check could reject a valid anchor.
                    .filter(|thread| thread.last_modified == last_thread.last_modified)
                    // Get the index of the anchor in the previous thread list
                    .map(|thread| thread.bump_index)
            });

            move |thread: &Thread, updates: &mut Vec<ThreadUpdate>| {
                match anchor_index {
                    // We found an anchor and will use it to determine if a thread has been
                    // deleted or bumped off.
                    Some(Some(anchor)) => {
                        if thread.bump_index < anchor {
                            // If the removed thread was before the anchor, it was deleted.
                            // Proof:
                            //   1. Two threads only swap orders when the bottom one is bumped.
                            //      (Stickying the bottom thread would also swap the order, but
                            //      that's too rare to worry about. Stickies should also update
                            //      last_modified, which will be caught by the check above.)
                            //   2. The anchor was not bumped and was behind the removed thread.
                            //   3. So, the anchor was always behind the removed thread.
                            //   4. Only the last thread on the board can be bumped off.
                            //   5. The removed thread could never have been the last thread on the
                            //      board, as the anchor was always behind it.
                            //   6. So, the removed thread could not have been bumped off.
                            updates.push(Deleted(thread.no));
                        } else {
                            // A thread after the anchor could have been the last thread at some
                            // point. So, we assume it was bumped off. Even if the thread lists are:
                            //        No. 1             No. 1
                            //  prev  No. 2  -->  curr  No. 2
                            //        No. 3
                            // We can't say for sure that No. 3 was deleted. Between polls, a new
                            // "phantom" thread could have been added, bumping off No. 3, and then
                            // deleted soon after. This could happen if our poll_interval is long or
                            // threads are being deleted quickly (e.g. in response to a raid).
                            updates.push(BumpedOff(thread.no));
                        }
                    }
                    // The board isn't empty, but we didn't find a valid anchor (maybe all threads
                    // are new or were modified). So, we assume all removed threads were bumped off.
                    Some(None) => {
                        updates.push(BumpedOff(thread.no));
                    }
                    // There is no last thread in the current list. So, the board is empty.
                    // Technically, we can't assume any threads were deleted because the phantom
                    // thread argument from above applies here. But, that would require an entire
                    // board getting deleted between polls, which is highly unlikely unless our poll
                    // interval is ridiculously long, in which case all bets are off since we're
                    // losing tons of data anyways. So, we assume that all threads were deleted.
                    None => {
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
                    error!("/{}/: Failed to fetch archive: {}", board, err)
                }),
        );
    }
}

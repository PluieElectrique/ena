use std::{
    cmp::Ordering,
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
        use ThreadUpdate::*;
        let mut updates = vec![];
        let mut removed = vec![];
        let anchor_no = curr_threads.last().map(|anchor| anchor.no);
        let mut found_anchor = false;

        // Sort ascending by no
        curr_threads.sort_by(|a, b| a.no.cmp(&b.no));

        let mut prev_iter = self.threads[&board].iter();
        let mut curr_iter = curr_threads.iter();
        let mut curr_thread = curr_iter.next();

        loop {
            match (prev_iter.next(), curr_thread) {
                (Some(prev), Some(curr)) => {
                    match prev.no.cmp(&curr.no) {
                        Ordering::Less => removed.push(prev),
                        Ordering::Equal => {
                            match prev.last_modified.cmp(&curr.last_modified) {
                                Ordering::Less => updates.push(Modified(curr.no)),
                                // We found an anchor: a thread which is not new and was not
                                // modified. See the comments below before `let anchor_index = ...`
                                // for a more detailed explanation of what this means.
                                Ordering::Equal => found_anchor = true,
                                Ordering::Greater => {
                                    // This should be an assert, but it seems that we can receive
                                    // old data even when using Last-Modified. So, we try to keep
                                    // running instead of crashing.
                                    error!(
                                        "/{}/ No. {} went back in time! Discarding this poll",
                                        board, prev.no
                                    );
                                    return;
                                }
                            }
                            curr_thread = curr_iter.next();
                        }
                        Ordering::Greater => {
                            // Again, bail instead of crashing.
                            error!(
                                "/{}/ Old thread No. {} reappeared! Discarding this poll",
                                board, prev.no
                            );
                            return;
                        }
                    }
                }
                (Some(prev), None) => {
                    removed.push(prev);
                }
                (None, Some(curr)) => {
                    updates.push(New(curr.no));
                    curr_thread = curr_iter.next();
                }
                (None, None) => break,
            }
        }

        // To determine if a removed thread was bumped off or deleted, we use the "anchor thread"
        // heuristic.
        //
        // Notes/Flaws:
        //   - When in doubt, we always assume that a removed thread was bumped off, as deletions
        //     are much rarer.
        //   - We will mark moved threads as deleted because they disappear from the board just like
        //     a deleted thread would.
        //   - If the last thread of a board is deleted, it will always be marked as bumped off. So,
        //     an entire board always deleted from the end will be completely marked as bumped off.
        //
        // An anchor thread is a thread which:
        //   1. Appears in the previous thread list (i.e. is not a new thread), and
        //   2. Has not been bumped since the last poll.
        //
        // Any thread which satisfies 1 and 2 is a valid anchor, but the last thread in the current
        // list is the best because it lets us catch as many deleted threads as possible.
        //
        // Now, if we find at least one anchor (found_anchor is set to true), the last thread in the
        // current list must also be an anchor. We can prove this:
        //
        // First, an axiom:
        //   Two threads only swap orders when the bottom one is bumped. (Stickying the bottom
        //   thread would also swap the order, but that's too rare to worry about.)
        //
        // Proof by contradiction:
        //   a. Assume the last thread is either new or was bumped. Then, at some point it must have
        //      been at the top of the thread list (disregarding stickies).
        //   b. For this thread to become the last thread, it must have swapped orders with the
        //      anchor we found. Thus, the anchor must have been bumped.
        //   c. But by definition, an anchor cannot have been bumped. Thus, the last thread cannot
        //      have been new or bumped.
        //
        // But, why can't we directly check that the last current thread wasn't modified and isn't
        // new? Well, we could, but saging a thread updates last_modified without bumping it. So, if
        // the last thread was saged, we would reject a valid anchor. With this method, we can prove
        // the last thread is a valid anchor even if it has been modified. We'll only reject a valid
        // anchor if every thread is saged between polls, which is very unlikely.
        let anchor_index = if self.threads.is_empty() {
            // Every thread is new, so we have no anchor.
            None
        } else if found_anchor {
            let anchor_no = anchor_no.unwrap();
            // We want the bump index of the anchor in the previous thread list.
            let anchor = self.threads[&board]
                .iter()
                .rev()
                .find(|thread| thread.no == anchor_no);
            match anchor {
                Some(anchor) => Some(anchor.bump_index),
                None => {
                    // I've made a logic mistake or false assumption about how threads work. Or,
                    // we've somehow received old data.
                    error!(
                        "/{}/ No. {} should be an anchor but is actually a new thread!",
                        board, anchor_no,
                    );
                    return;
                }
            }
        } else {
            None
        };

        for thread in &removed {
            match anchor_index {
                // We found an anchor and will use it to determine if a thread has been deleted or
                // bumped off.
                Some(anchor) => {
                    if thread.bump_index < anchor {
                        // If the removed thread was previously before the anchor, it was deleted.
                        // Recall the axiom from above:
                        //   Two threads only swap orders when the bottom one is bumped. (Stickying
                        //   the bottom thread would also swap the order, but that's too rare to
                        //   worry about.)
                        // Proof:
                        //   a. The anchor was not bumped and was behind the removed thread.
                        //   b. So, the anchor was always behind the removed thread.
                        //   c. Only the last thread on the board can be bumped off.
                        //   d. The removed thread could never have been the last thread on the
                        //      board, as the anchor was always behind it.
                        //   e. So, the removed thread could not have been bumped off.
                        updates.push(Deleted(thread.no));
                    } else {
                        // A thread after the anchor could have been the last thread at some point.
                        // So, we assume it was bumped off. Even if the thread lists are:
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
                // We didn't find a valid anchor (maybe all threads are new or were modified). So,
                // we assume all removed threads were bumped off.
                None => {
                    updates.push(BumpedOff(thread.no));
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
                "/{}/: Updating {} thread{} ({})",
                board,
                len,
                if len == 1 { "" } else { "s" },
                nonzero_list_format!(
                    "{} new",
                    new,
                    "{} modified",
                    modified,
                    "{} bumped off",
                    bumped_off,
                    "{} deleted",
                    deleted,
                ),
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

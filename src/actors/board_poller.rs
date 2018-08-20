use std::time::Duration;

use actix::prelude::*;
use futures::prelude::*;

use four_chan::fetcher::{FetchError, FetchThreads, Fetcher};
use four_chan::{Board, Thread};

pub struct BoardPoller {
    board: Board,
    threads: Vec<Thread>,
    interval: u64,
    deleted_page_threshold: u8,
    subscribers: Vec<Recipient<BoardUpdate>>,
    fetcher: Addr<Fetcher>,
}

impl Actor for BoardPoller {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        self.poll(0, ctx);
    }
}

impl BoardPoller {
    pub fn new(
        board: Board,
        interval: u64,
        deleted_page_threshold: u8,
        subscribers: Vec<Recipient<BoardUpdate>>,
        fetcher: Addr<Fetcher>,
    ) -> Self {
        Self {
            board,
            threads: vec![],
            interval,
            deleted_page_threshold,
            subscribers,
            fetcher,
        }
    }

    fn update_threads(&mut self, mut curr_threads: Vec<Thread>) {
        use self::ThreadUpdate::*;
        let mut updates = vec![];
        let max_threads = if Board::f == self.board { 30 } else { 150 };
        let mut new_threads = false;

        let push_removed = {
            // If there were and are less than the maximum number of threads, any "bumped off"
            // thread is very likely a deletion.
            let less_than_max =
                curr_threads.len() < max_threads && self.threads.len() < max_threads;
            let threshold = self.deleted_page_threshold;
            let last_no = curr_threads[curr_threads.len() - 1].no;
            let last_index = self
                .threads
                .iter()
                .rev()
                .find(|thread| thread.no == last_no)
                .map_or(0, |thread| thread.bump_index);

            move |thread: &Thread, updates: &mut Vec<ThreadUpdate>| {
                if thread.bump_index < last_index || thread.page <= threshold || less_than_max {
                    updates.push(Deleted(thread.no));
                } else {
                    updates.push(BumpedOff(thread.no));
                }
            }
        };

        // Sort ascending by no
        curr_threads.sort_by(|a, b| a.no.cmp(&b.no));

        {
            let mut prev_iter = self.threads.iter();
            let mut curr_iter = curr_threads.iter();

            let mut prev_thread = prev_iter.next();
            let mut curr_thread = curr_iter.next();

            loop {
                match (prev_thread, curr_thread) {
                    (Some(prev), Some(curr)) => {
                        if prev.no < curr.no {
                            push_removed(prev, &mut updates);
                            prev_thread = prev_iter.next();
                        } else if prev.no == curr.no {
                            if prev.last_modified < curr.last_modified {
                                updates.push(Modified(curr.no));
                            } else if prev.last_modified > curr.last_modified {
                                warn!(
                                    "Got old data for /{}/, skipping rest of threads.json",
                                    self.board
                                );
                                return;
                            }
                            prev_thread = prev_iter.next();
                            curr_thread = curr_iter.next();
                        } else if prev.no > curr.no {
                            warn!(
                                "Got old data for /{}/, skipping rest of threads.json",
                                self.board
                            );
                            return;
                        }
                    }
                    (Some(prev), None) => {
                        push_removed(prev, &mut updates);
                        prev_thread = prev_iter.next();
                    }
                    (None, Some(curr)) => {
                        updates.push(New(curr.no));
                        new_threads = true;
                        curr_thread = curr_iter.next();
                    }
                    (None, None) => break,
                }
            }
        }

        // If the thread count has decreased but there are no new threads, then any "bumped off"
        // threads was very likely deleted.
        if self.threads.len() == max_threads && curr_threads.len() < max_threads && !new_threads {
            for update in &mut updates {
                if let BumpedOff(no) = *update {
                    *update = Deleted(no);
                }
            }
        }

        debug!(
            "Updating {} threads from /{}/: {:?}",
            updates.len(),
            self.board,
            updates
        );

        for subscriber in &self.subscribers {
            Arbiter::spawn(
                subscriber
                    .send(BoardUpdate(updates.clone()))
                    .map_err(|err| error!("{}", err)),
            );
        }
        self.threads = curr_threads;
    }

    fn poll(&self, interval: u64, ctx: &mut Context<Self>) {
        ctx.run_later(Duration::new(interval, 0), |act, ctx| {
            ctx.spawn(
                act.fetcher
                    .send(FetchThreads(act.board))
                    .map_err(|err| log_error!(&err))
                    .into_actor(act)
                    .map(|threads, act, ctx| {
                        match threads {
                            Ok(threads) => {
                                act.update_threads(threads);
                                debug!("Fetched and updated threads from /{}/", act.board);
                            }
                            Err(err) => match err {
                                FetchError::NotModified => {}
                                _ => error!("{}", err),
                            },
                        }
                        act.poll(act.interval, ctx);
                    }),
            );
        });
    }
}

#[derive(Message)]
pub struct BoardUpdate(pub Vec<ThreadUpdate>);

#[derive(Clone, Debug)]
pub enum ThreadUpdate {
    New(u64),
    Modified(u64),
    BumpedOff(u64),
    Deleted(u64),
}

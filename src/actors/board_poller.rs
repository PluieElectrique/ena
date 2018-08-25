use std::time::Duration;

use actix::prelude::*;
use chrono::prelude::*;
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

    fn update_threads(&mut self, mut curr_threads: Vec<Thread>, last_modified: DateTime<Utc>) {
        use self::ThreadUpdate::*;
        let mut updates = vec![];
        let max_threads = self.board.max_threads() as usize;
        let mut new_threads = false;

        let push_removed = {
            // If there were and now are less than the maximum number of threads, any removed thread
            // is likely a deletion.
            let less_than_max =
                curr_threads.len() < max_threads && self.threads.len() < max_threads;
            let threshold = self.deleted_page_threshold;
            let last_no = curr_threads[curr_threads.len() - 1].no;
            let anchor_index = self
                .threads
                .iter()
                .rev()
                .find(|thread| thread.no == last_no)
                .map(|thread| thread.bump_index);

            move |thread: &Thread, updates: &mut Vec<ThreadUpdate>| {
                if anchor_index.is_none() {
                    // If all of the threads have changed, then we probably loaded the thread list
                    // from the database, and can't assume that any of the old threads were deleted
                    updates.push(BumpedOff(thread.no));
                } else if less_than_max
                    || thread.bump_index < anchor_index.unwrap()
                    || thread.page <= threshold
                {
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
                        new_threads = true;
                        updates.push(New(curr.no));
                        curr_thread = curr_iter.next();
                    }
                    (None, None) => break,
                }
            }
        }

        // If the thread count has decreased but there are no new threads, then any "bumped off"
        // threads was likely deleted.
        if self.threads.len() == max_threads && curr_threads.len() < max_threads && !new_threads {
            for update in &mut updates {
                if let BumpedOff(no) = *update {
                    *update = Deleted(no);
                }
            }
        }

        debug!(
            "Updating {} thread(s) from /{}/: {:?}",
            updates.len(),
            self.board,
            updates
        );

        for subscriber in &self.subscribers {
            Arbiter::spawn(
                subscriber
                    .send(BoardUpdate(updates.clone(), last_modified))
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
                    .map(|res, act, ctx| {
                        match res {
                            Ok((threads, last_modified)) => {
                                act.update_threads(threads, last_modified);
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
pub struct BoardUpdate(pub Vec<ThreadUpdate>, pub DateTime<Utc>);

#[derive(Clone, Debug)]
pub enum ThreadUpdate {
    New(u64),
    Modified(u64),
    BumpedOff(u64),
    Deleted(u64),
}

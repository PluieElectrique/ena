use std::time::Duration;

use actix::prelude::*;
use futures::prelude::*;

use four_chan;
use four_chan::fetcher::{FetchError, FetchThreads, Fetcher};
use {log_error, log_warn};

pub struct BoardPoller {
    board: four_chan::Board,
    threads: Vec<four_chan::Thread>,
    interval: u64,
    subscribers: Vec<Recipient<BoardUpdate>>,
}

impl Actor for BoardPoller {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        self.poll(ctx);
    }
}

impl BoardPoller {
    pub fn new(
        board: four_chan::Board,
        interval: u64,
        subscribers: Vec<Recipient<BoardUpdate>>,
    ) -> Self {
        Self {
            board,
            threads: vec![],
            interval,
            subscribers,
        }
    }

    fn update_threads(&mut self, mut curr_threads: Vec<four_chan::Thread>) {
        let mut new_threads = vec![];
        let mut modified_threads = vec![];
        let mut removed_threads = vec![];

        // Sort ascending
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
                            removed_threads.push(prev.no);
                            prev_thread = prev_iter.next();
                        } else if prev.no == curr.no {
                            if prev.last_modified < curr.last_modified {
                                modified_threads.push(curr.no);
                            }
                            prev_thread = prev_iter.next();
                            curr_thread = curr_iter.next();
                        } else if prev.no > curr.no {
                            // Is it possible for an old thread to be added back?
                            // TODO: better error handling/logging
                            println!(
                                "A previously removed thread ({}) from /{}/ has reappeared!",
                                curr.no, self.board
                            );
                            new_threads.push(curr.no);
                            curr_thread = curr_iter.next();
                        }
                    }
                    (Some(prev), None) => {
                        removed_threads.push(prev.no);
                        prev_thread = prev_iter.next();
                    }
                    (None, Some(curr)) => {
                        new_threads.push(curr.no);
                        curr_thread = curr_iter.next();
                    }
                    (None, None) => break,
                }
            }
            println!("New threads: {:?}", new_threads);
            println!("Modified threads: {:?}", modified_threads);
            println!("Removed threads: {:?}", removed_threads);
        }
        self.threads = curr_threads;
    }

    fn poll(&self, ctx: &mut Context<Self>) {
        ctx.run_later(Duration::new(self.interval, 0), |act, ctx| {
            let fetcher = System::current().registry().get::<Fetcher>();
            ctx.spawn(
                fetcher
                    .send(FetchThreads(act.board))
                    .map_err(|err| log_error(&err))
                    .into_actor(act)
                    .map(|threads, act, _ctx| match threads {
                        Ok(threads) => {
                            act.update_threads(threads);
                            debug!("Fetched and updated threads from {}", act.board);
                        }
                        Err(err) => {
                            if let FetchError::NotModified = err {
                                log_warn(&err);
                            } else {
                                log_error(&err);
                            }
                        }
                    }),
            );
            act.poll(ctx);
        });
    }
}

#[derive(Message)]
pub enum BoardUpdate {
    NewThread(u64),
    DeletedThread(u64),
    ModifiedThread(u64),
}

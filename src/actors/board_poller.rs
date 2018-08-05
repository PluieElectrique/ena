use std::time::Duration;

use actix::prelude::*;
use futures::prelude::*;

use four_chan;

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

    fn poll(&self, ctx: &mut Context<Self>) {
        ctx.run_later(Duration::new(self.interval, 0), |act, ctx| {
            let fetcher = System::current().registry().get::<four_chan::Fetcher>();
            Arbiter::spawn(
                fetcher
                    .send(four_chan::FetchThreads(act.board))
                    .and_then(|threads| {
                        let now = ::std::time::Instant::now();
                        println!("Fetched threads at {:?}\n{:?}", now, threads);
                        Ok(())
                    })
                    .map_err(|e| println!("{}", e)),
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

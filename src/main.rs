extern crate actix;
extern crate ena;
extern crate futures;
extern crate hyper;
extern crate hyper_tls;
extern crate mysql_async as my;
extern crate serde_json;
extern crate tokio_core;

use actix::prelude::*;
use ena::actors::*;
use ena::*;
use futures::future;
use futures::prelude::*;
use my::prelude::*;
use tokio_core::reactor::Core;

fn main() {
    let config = parse_config().expect("Couldn't read config file");
    let board_sql = BOARD_SQL.replace("%%CHARSET%%", &config.charset);

    let mut core = Core::new().expect("Couldn't create Tokio core");
    let pool = my::Pool::new(config.database_url, &core.handle());

    let sys = System::new("ena");
    BoardPoller::new(config.boards[0], config.poll_interval, vec![]).start();

    let futures = {
        let pool = pool.clone();

        config.boards.iter().map(move |board| {
            println!("Showing 5 posts from {}", board);

            let board_sql = board_sql.replace("%%BOARD%%", &board.to_string());
            pool.get_conn()
                .and_then(|conn| conn.drop_query(board_sql))
                .and_then(move |conn| conn.query(format!("SELECT title FROM {} LIMIT 5", board)))
                .and_then(|result| {
                    result.for_each_and_drop(|row| {
                        let (title,): (Option<String>,) = my::from_row(row);
                        println!("{:?}", title);
                    })
                })
                .and_then(|conn| conn.disconnect())
        })
    };

    let future = future::join_all(futures).and_then(|_| pool.disconnect());

    core.run(future).expect("Failed to run future");

    sys.run();
}

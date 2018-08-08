extern crate actix;
#[macro_use]
extern crate ena;
extern crate failure;
extern crate futures;
extern crate hyper;
extern crate hyper_tls;
#[macro_use]
extern crate log;
extern crate mysql_async as my;
extern crate pretty_env_logger;
extern crate serde_json;
extern crate tokio;

use std::process;

use actix::prelude::*;
use ena::actors::*;
use ena::*;
use futures::prelude::*;
use my::prelude::*;

fn main() {
    pretty_env_logger::init();

    let config = parse_config().unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    });
    let board = config.boards[0];

    let pool = my::Pool::new(config.database_url);

    let sys = System::new("ena");
    BoardPoller::new(board, config.poll_interval, vec![]).start();

    println!("Showing 5 posts from {}", board);

    let board_sql = BOARD_SQL
        .replace(CHARSET_REPLACE, &config.charset)
        .replace(BOARD_REPLACE, &board.to_string());
    let future = pool
        .get_conn()
        .and_then(|conn| conn.drop_query(board_sql))
        .and_then(move |conn| conn.query(format!("SELECT title FROM {} LIMIT 5", board)))
        .and_then(|result| {
            result.for_each_and_drop(|row| {
                let (title,): (Option<String>,) = my::from_row(row);
                println!("{:?}", title);
            })
        })
        .and_then(|conn| conn.disconnect())
        .and_then(|_| pool.disconnect())
        .map_err(|err| println!("{}", err));

    tokio::run(future);

    sys.run();
}

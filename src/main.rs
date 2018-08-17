extern crate actix;
#[macro_use]
extern crate ena;
extern crate failure;
#[macro_use]
extern crate log;
extern crate mysql_async as my;
extern crate pretty_env_logger;

use std::process;

use actix::prelude::*;
use ena::actors::*;
use ena::*;

use four_chan::fetcher::Fetcher;

fn main() {
    pretty_env_logger::init();

    let config = parse_config().unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    });

    let sys = System::new("ena");

    let fetcher = Fetcher::new().start();

    let database = Database::new(
        my::Pool::new(config.database_url),
        config.adjust_timestamps,
        &config.boards,
        &config.charset,
    ).unwrap_or_else(|err| {
        error!("Database initialization error: {}", err);
        process::exit(1);
    }).start();

    for board in config.boards {
        let thread_updater = ThreadUpdater::new(board, database.clone(), fetcher.clone()).start();

        // TODO: Should BoardPoller be able to handle multiple boards?
        BoardPoller::new(
            board,
            config.poll_interval,
            config.deleted_page_threshold,
            vec![thread_updater.recipient()],
            fetcher.clone(),
        ).start();
    }

    sys.run();
}

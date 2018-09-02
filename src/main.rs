extern crate actix;
#[macro_use]
extern crate ena;
extern crate failure;
#[macro_use]
extern crate log;
extern crate mysql_async as my;
extern crate pretty_env_logger;

use std::process;
use std::time::Duration;

use actix::prelude::*;
use ena::actors::*;
use ena::*;

fn main() {
    pretty_env_logger::init();

    let config = parse_config().unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    });

    let sys = System::new("ena");

    let database = Database::new(
        my::Pool::new(config.database_url),
        &config.boards,
        &config.charset,
        config.adjust_timestamps,
        config.create_index_counters,
    ).unwrap_or_else(|err| {
        error!("Database initialization error: {}", err);
        process::exit(1);
    }).start();

    let fetcher = Fetcher::new(
        Duration::from_millis(config.fetch_delay),
        config.media_path,
        &config.boards,
    ).unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    }).start();

    for board in config.boards {
        let thread_updater = ThreadUpdater::new(
            board,
            database.clone(),
            fetcher.clone(),
            config.refetch_archived_threads,
            config.always_add_archive_times,
        ).start();

        BoardPoller::new(board, config.poll_interval, thread_updater, fetcher.clone()).start();
    }

    sys.run();
}

extern crate actix;
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
    info!("Ena is starting");

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
        config.download_media,
        config.download_thumbs,
    ).unwrap_or_else(|err| {
        error!("Database initialization error: {}", err);
        process::exit(1);
    });
    let database = Arbiter::builder()
        .stop_system_on_panic(true)
        .start(|_| database);

    let fetcher = Fetcher::new(
        &config.media_path,
        &config.media_rate_limiting,
        &config.thread_rate_limiting,
        &config.thread_list_rate_limiting,
    ).unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    }).start();

    let thread_updater = ThreadUpdater::new(
        database,
        fetcher.clone(),
        config.refetch_archived_threads,
        config.always_add_archive_times,
    ).start();

    BoardPoller::new(
        &config.boards,
        Duration::from_secs(config.poll_interval),
        thread_updater,
        fetcher,
    ).start();

    info!("Ena is running");
    sys.run();
}

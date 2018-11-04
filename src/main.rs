extern crate actix;
extern crate ena;
extern crate env_logger;
extern crate failure;
#[macro_use]
extern crate log;
extern crate mysql_async as my;

use std::io::Write;
use std::process;
use std::time::Duration;

use actix::prelude::*;
use ena::actors::*;
use ena::config::parse_config;
use ena::log_error;

const THREAD_UPDATER_MAILBOX_CAPACITY: usize = 500;

fn main() {
    env_logger::Builder::from_default_env()
        .format(|fmt, record| {
            let level = record.level();
            let level_style = fmt.default_level_style(level);
            let timestamp = fmt.timestamp();
            let args = record.args();

            writeln!(
                fmt,
                "{:>5} {} >    {}",
                level_style.value(level),
                timestamp,
                args
            )
        }).init();

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

    let thread_updater_ctx = {
        let (_, receiver) = actix::dev::channel::channel(THREAD_UPDATER_MAILBOX_CAPACITY);
        Context::with_receiver(receiver)
    };

    let fetcher = Fetcher::create(
        &config.media_path,
        &config.media_rate_limiting,
        &config.thread_rate_limiting,
        &config.thread_list_rate_limiting,
        thread_updater_ctx.address(),
    ).unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    });

    let thread_updater = thread_updater_ctx.run(ThreadUpdater::new(
        database,
        fetcher.clone(),
        config.refetch_archived_threads,
        config.always_add_archive_times,
    ));

    BoardPoller::new(
        &config.boards,
        Duration::from_secs(config.poll_interval),
        config.fetch_archive,
        thread_updater,
        fetcher,
    ).start();

    info!("Ena is running");
    sys.run();
}

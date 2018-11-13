extern crate actix;
extern crate ena;
extern crate env_logger;
extern crate failure;
#[macro_use]
extern crate log;
extern crate mysql_async as my;

use std::io::Write;
use std::process;

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
        my::Pool::new(config.database_media.database_url),
        &config.scraping.boards,
        &config.database_media.charset,
        config.asagi_compat.adjust_timestamps,
        config.asagi_compat.create_index_counters,
        config.scraping.download_media,
        config.scraping.download_thumbs,
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
        &config.database_media.media_path,
        &config.network.rate_limiting.media,
        &config.network.rate_limiting.thread,
        &config.network.rate_limiting.thread_list,
        thread_updater_ctx.address(),
    ).unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    });

    let thread_updater = thread_updater_ctx.run(ThreadUpdater::new(
        database,
        fetcher.clone(),
        config.asagi_compat.refetch_archived_threads,
        config.asagi_compat.always_add_archive_times,
    ));

    BoardPoller::new(
        &config.scraping.boards,
        config.scraping.poll_interval,
        config.scraping.fetch_archive,
        thread_updater,
        fetcher,
    ).start();

    info!("Ena is running");
    sys.run();
}

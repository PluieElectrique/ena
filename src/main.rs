extern crate actix;
extern crate ena;
extern crate env_logger;
extern crate failure;
#[macro_use]
extern crate log;

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
            let timestamp = fmt.timestamp();
            let level = record.level();
            let level_style = fmt.default_level_style(level);
            let args = record.args();

            writeln!(
                fmt,
                "{} {:<5} >    {}",
                timestamp,
                level_style.value(level),
                args
            )
        }).init();

    info!("Ena is starting");

    let config = parse_config().unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    });

    let sys = System::new("ena");

    let database = {
        let database = Database::new(&config).unwrap_or_else(|err| {
            error!("Database initialization error: {}", err);
            process::exit(1);
        });
        Arbiter::builder()
            .stop_system_on_panic(true)
            .start(|_| database)
    };

    let thread_updater_ctx = {
        let (_, receiver) = actix::dev::channel::channel(THREAD_UPDATER_MAILBOX_CAPACITY);
        Context::with_receiver(receiver)
    };

    let fetcher = Fetcher::create(&config, thread_updater_ctx.address()).unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    });

    let thread_updater =
        thread_updater_ctx.run(ThreadUpdater::new(&config, database, fetcher.clone()));

    BoardPoller::new(&config, thread_updater, fetcher).start();

    info!("Ena is running");
    sys.run();
}

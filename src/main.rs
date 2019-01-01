use std::io::Write;
use std::process;

use actix::prelude::*;
use log::{error, info};

use ena::{actors::*, config::parse_config, log_error};

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
        })
        .init();

    info!("Ena is starting");

    let config = parse_config().unwrap_or_else(|err| {
        log_error!(err.as_fail());
        process::exit(1);
    });

    let sys = System::new("ena");

    let database = {
        let database = Database::try_new(&config).unwrap_or_else(|err| {
            error!("Database initialization error: {}", err);
            process::exit(1);
        });
        Arbiter::builder()
            .stop_system_on_panic(true)
            .start(|_| database)
    };

    // To create ThreadUpdater, we need Addr<Fetcher>. But to create Fetcher, we need
    // Addr<ThreadUpdater>! To solve this circular dependency, we first create ThreadUpdater's
    // Context. This gives us Addr<ThreadUpdater> without having to create ThreadUpdater. We then
    // use this Addr to create Fetcher, which gives us Addr<Fetcher>. Finally, we pass this Addr to
    // ThreadUpdater::new, and run ThreadUpdater in its previously created Context.
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

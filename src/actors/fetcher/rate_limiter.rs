use std::fmt;
use std::time::{Duration, Instant};

use futures::prelude::*;
use futures::stream::{Fuse, FuturesUnordered};
use tokio::timer::Delay;

use config::RateLimitingSettings;

/// An adapter for a stream of futures which limits the number of concurrently running futures and
/// the number of futures that run in a given time interval. Results are returned in the order that
/// the futures complete.
#[must_use = "streams do nothing unless polled"]
pub struct RateLimiter<S>
where
    S: Stream,
    S::Item: IntoFuture,
{
    stream: Fuse<S>,
    queue: FuturesUnordered<<S::Item as IntoFuture>::Future>,
    delay: Option<Delay>,
    interval: Duration,

    /// The number of futures which have run in the current interval
    curr_interval: usize,

    /// The maximum number of futures which can run in a given interval
    max_interval: usize,

    /// The maximum number of futures which can run at the same time
    max_concurrent: usize,
}

impl<S> RateLimiter<S>
where
    S: Stream,
    S::Item: IntoFuture<Error = <S as Stream>::Error>,
{
    pub fn new(s: S, settings: &RateLimitingSettings) -> Self {
        Self {
            stream: s.fuse(),
            queue: FuturesUnordered::new(),
            delay: None,
            interval: settings.interval,
            curr_interval: 0,
            max_interval: settings.max_interval,
            max_concurrent: settings.max_concurrent,
        }
    }
}

impl<S> fmt::Debug for RateLimiter<S>
where
    S: Stream + fmt::Debug,
    S::Item: IntoFuture,
    <<S as Stream>::Item as IntoFuture>::Future: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("RateLimiter")
            .field("stream", &self.stream)
            .field("queue", &self.queue)
            .field("delay", &self.delay)
            .field("interval", &self.interval)
            .field("curr_interval", &self.curr_interval)
            .field("max_interval", &self.max_interval)
            .field("max_concurrent", &self.max_concurrent)
            .finish()
    }
}

impl<S> Stream for RateLimiter<S>
where
    S: Stream,
    S::Item: IntoFuture<Error = <S as Stream>::Error>,
{
    type Item = <S::Item as IntoFuture>::Item;
    type Error = <S as Stream>::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        // Reset our interval count if the Delay has elapsed
        if let Some(res) = self.delay.as_mut().map(|delay| delay.poll()) {
            match res {
                Ok(Async::Ready(())) => {
                    self.curr_interval = 0;
                    self.delay = None;
                }
                Ok(Async::NotReady) => {}
                Err(err) => {
                    // There's not much we can do here. If the timer has shutdown, something has
                    // really gone wrong. If the timer is at capacity, something has also gone
                    // wrong, and we can't shed load because that would require dropping ourselves.
                    panic!("Timer error: {}", err);
                }
            }
        }

        // Queue up as many futures as we can
        while self.queue.len() < self.max_concurrent && self.curr_interval < self.max_interval {
            let future = match self.stream.poll()? {
                Async::Ready(Some(s)) => s.into_future(),
                Async::Ready(None) | Async::NotReady => break,
            };

            self.curr_interval += 1;
            self.queue.push(future);
        }

        // Set up the next Delay if one currently isn't running
        if self.delay.is_none() && self.curr_interval > 0 {
            self.delay = Some(Delay::new(Instant::now() + self.interval));
        }

        // Try polling a new future
        if let Some(val) = try_ready!(self.queue.poll()) {
            return Ok(Async::Ready(Some(val)));
        }

        // If we've gotten this far, then there are no events for us to process and nothing was
        // ready, so figure out if we're not done yet or if we've reached the end.
        if self.stream.is_done() {
            Ok(Async::Ready(None))
        } else {
            Ok(Async::NotReady)
        }
    }
}

/// A stream-to-future adapter which polls a "task" stream to completion.
#[must_use = "futures do nothing unless polled"]
pub struct Consume<S>
where
    S: Stream<Item = (), Error = ()>,
{
    stream: S,
}

impl<S> Consume<S>
where
    S: Stream<Item = (), Error = ()>,
{
    pub fn new(s: S) -> Self {
        Self { stream: s }
    }
}

impl<S> Future for Consume<S>
where
    S: Stream<Item = (), Error = ()>,
{
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        loop {
            match self.stream.poll() {
                Ok(Async::Ready(Some(_))) => {}
                Ok(Async::Ready(None)) => return Ok(Async::Ready(())),
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(_) => {}
            }
        }
    }
}

pub trait StreamExt: Sized {
    fn consume(self) -> Consume<Self>
    where
        Self: Stream<Item = (), Error = ()>;

    fn rate_limit(self, settings: &RateLimitingSettings) -> RateLimiter<Self>
    where
        Self: Stream,
        <Self as Stream>::Item: IntoFuture<Error = <Self as Stream>::Error>;
}

impl<T> StreamExt for T
where
    T: Sized,
{
    fn consume(self) -> Consume<Self>
    where
        Self: Stream<Item = (), Error = ()>,
    {
        Consume::new(self)
    }

    fn rate_limit(self, settings: &RateLimitingSettings) -> RateLimiter<Self>
    where
        Self: Stream,
        <Self as Stream>::Item: IntoFuture<Error = <Self as Stream>::Error>,
    {
        RateLimiter::new(self, settings)
    }
}

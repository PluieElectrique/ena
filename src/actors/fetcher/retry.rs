use std::time::Duration;

use futures::{
    prelude::*,
    stream::Fuse,
    sync::mpsc::{self, Receiver, Sender},
};

use tokio::timer::DelayQueue;

use crate::config::RetryBackoffConfig;

/// A struct which represents a request that can be retried
pub struct Retry<T> {
    data: T,
    delay: Duration,
    factor: u32,
    max: Duration,
}

impl<T> Retry<T> {
    pub fn new(data: T, config: &RetryBackoffConfig) -> Self {
        Self {
            data,
            delay: config.base,
            factor: config.factor,
            max: config.max,
        }
    }

    pub fn can_retry(&self) -> bool {
        self.delay <= self.max
    }

    pub fn as_data(&self) -> &T {
        &self.data
    }

    pub fn to_data(&self) -> T
    where
        T: Clone,
    {
        self.data.clone()
    }

    pub fn into_data(self) -> T {
        self.data
    }
}

/// A wrapper around a `DelayQueue` which inserts requests from a `Stream` of `Retry`.
///
/// # Panics
/// The queue will panic if a `Retry` has exceeded the maximum delay and can no longer be retried.
pub struct RetryQueue<S, T>
where
    S: Stream<Item = Retry<T>, Error = ()>,
{
    stream: Fuse<S>,
    queue: DelayQueue<Retry<T>>,
}

impl<S, T> RetryQueue<S, T>
where
    S: Stream<Item = Retry<T>, Error = ()>,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream: stream.fuse(),
            queue: DelayQueue::new(),
        }
    }
}

impl<S, T> Stream for RetryQueue<S, T>
where
    S: Stream<Item = Retry<T>, Error = ()>,
{
    type Item = Retry<T>;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut stream_done = false;

        loop {
            match self.stream.poll()? {
                Async::Ready(Some(mut retry)) => {
                    assert!(retry.can_retry());
                    let delay = retry.delay;
                    retry.delay *= retry.factor;
                    self.queue.insert(retry, delay);
                }
                Async::NotReady => break,
                Async::Ready(None) => {
                    stream_done = true;
                    break;
                }
            };
        }

        match self.queue.poll() {
            Ok(Async::Ready(Some(value))) => Ok(Async::Ready(Some(value.into_inner()))),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(None)) => {
                if stream_done {
                    Ok(Async::Ready(None))
                } else {
                    Ok(Async::NotReady)
                }
            }
            Err(err) => {
                // There's not much we can do here. If the timer has shutdown, something has really
                // gone wrong. If the timer is at capacity, something has also gone wrong, and we
                // can't shed load because that would require dropping ourselves.
                panic!("Timer error: {}", err);
            }
        }
    }
}

pub fn retry_channel<T>(buffer: usize) -> (Sender<Retry<T>>, RetryQueue<Receiver<Retry<T>>, T>) {
    let (sender, receiver) = mpsc::channel(buffer);
    (sender, RetryQueue::new(receiver))
}

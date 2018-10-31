use std::time::Duration;

use futures::prelude::*;
use futures::stream::Fuse;
use tokio;

/// A wrapper around a `tokio::timer::DelayQueue` which inserts items polled from a `Stream`.
pub struct DelayQueue<S, T>
where
    S: Stream<Item = (T, Duration), Error = ()>,
{
    stream: Fuse<S>,
    queue: tokio::timer::DelayQueue<T>,
}

impl<S, T> DelayQueue<S, T>
where
    S: Stream<Item = (T, Duration), Error = ()>,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream: stream.fuse(),
            queue: tokio::timer::DelayQueue::new(),
        }
    }
}

impl<S, T> Stream for DelayQueue<S, T>
where
    S: Stream<Item = (T, Duration), Error = ()>,
{
    type Item = T;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut stream_done = false;

        loop {
            match self.stream.poll()? {
                Async::Ready(Some((value, duration))) => self.queue.insert(value, duration),
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
            Ok(Async::Ready(None)) => if stream_done {
                Ok(Async::Ready(None))
            } else {
                Ok(Async::NotReady)
            },
            Err(err) => {
                // There's not much we can do here. If the timer has shutdown, something has really
                // gone wrong. If the timer is at capacity, something has also gone wrong, and we
                // can't shed load because that would require dropping ourselves.
                panic!("Timer error: {}", err);
            }
        }
    }
}

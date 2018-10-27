use actix::dev::MessageResponse;
use actix::prelude::*;
use futures::sync::mpsc::Sender;

use super::*;
use four_chan::Board;

pub trait ToUri {
    fn to_uri(&self) -> Uri;
}

/// A key for `Fetcher`'s last modified hashmap. `LastModifiedKey(board, Some(no))` represents a
/// thread and `LastModifiedKey(board, None)` represents the `threads.json` of that board.
#[derive(Debug, Eq, Hash, PartialEq)]
pub struct LastModifiedKey(Board, Option<u64>);

impl<'a> From<&'a (Board, u64)> for LastModifiedKey {
    fn from(msg: &(Board, u64)) -> Self {
        LastModifiedKey(msg.0, Some(msg.1))
    }
}

impl<'a> From<&'a FetchThread> for LastModifiedKey {
    fn from(msg: &FetchThread) -> Self {
        LastModifiedKey(msg.0, Some(msg.1))
    }
}

impl<'a> From<&'a FetchThreadList> for LastModifiedKey {
    fn from(msg: &FetchThreadList) -> Self {
        LastModifiedKey(msg.0, None)
    }
}

/// An Actix `MessageResponse` which lets us queue a future in our `RateLimiter`.
pub struct RateLimitedResponse<I, E> {
    pub sender: Sender<Box<Future<Item = (), Error = ()>>>,
    pub future: Box<Future<Item = I, Error = E>>,
}

impl<A, M, I: 'static, E: 'static> MessageResponse<A, M> for RateLimitedResponse<I, E>
where
    A: Actor,
    M: Message<Result = Result<I, E>>,
{
    fn handle<R: ResponseChannel<M>>(self, _: &mut A::Context, tx: Option<R>) {
        Arbiter::spawn(
            self.sender
                .send(Box::new(self.future.then(move |res| {
                    if let Some(tx) = tx {
                        tx.send(res);
                    }
                    Ok(())
                }))).map(|_| ())
                .map_err(|err| error!("Failed to send RateLimitedResponse future: {}", err)),
        )
    }
}

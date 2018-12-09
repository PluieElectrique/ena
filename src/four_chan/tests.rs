#![cfg(test)]

use failure::Error;
use futures::prelude::*;
use hyper::{Body, Client};
use hyper_tls::HttpsConnector;
use serde_derive::Deserialize;
use tokio::runtime::Runtime;

use super::{num_to_bool, API_URI_PREFIX};

#[derive(Deserialize)]
struct BoardsWrapper {
    boards: Vec<Board>,
}

#[derive(Deserialize)]
struct Board {
    board: super::Board,
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    is_archived: bool,
}

#[test]
fn boards_json() -> Result<(), Error> {
    let mut runtime = Runtime::new()?;
    let https = HttpsConnector::new(1)?;
    let client = Client::builder().build::<_, Body>(https);

    let uri = format!("{}/boards.json", API_URI_PREFIX).parse()?;

    let boards: Result<Vec<Board>, Error> = runtime.block_on(
        client
            .get(uri)
            .from_err()
            .and_then(|res| res.into_body().concat2().from_err())
            .and_then(|body| {
                let BoardsWrapper { boards } = serde_json::from_slice(&body)?;
                Ok(boards)
            }),
    );
    runtime.shutdown_now().wait().unwrap();

    for Board { board, is_archived } in boards? {
        assert_eq!(
            board.is_archived(),
            is_archived,
            "/{}/'s correct archive status is {}",
            board,
            is_archived,
        );
    }
    Ok(())
}

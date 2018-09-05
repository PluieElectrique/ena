#![cfg(test)]

use std::str;

use failure::Error;
use futures::prelude::*;
use hyper::client::Client;
use hyper::Body;
use hyper_tls::HttpsConnector;
use regex::Regex;
use serde_json;
use tokio::runtime::Runtime;

use super::{num_to_bool, API_URI_PREFIX};

#[derive(Deserialize)]
struct BoardsWrapper {
    boards: Vec<Board>,
}

#[derive(Deserialize)]
struct Board {
    board: super::Board,
    per_page: usize,
    pages: usize,
    #[serde(deserialize_with = "num_to_bool")]
    #[serde(default)]
    is_archived: bool,
}

#[test]
fn boards_json() {
    let mut runtime = Runtime::new().unwrap();
    let https = HttpsConnector::new(1).unwrap();
    let client = Client::builder().build::<_, Body>(https);

    let uri = format!("{}/boards.json", API_URI_PREFIX).parse().unwrap();

    let boards: Result<Vec<Board>, Error> = runtime.block_on(
        client
            .get(uri)
            .from_err()
            .and_then(|res| res.into_body().concat2().from_err())
            .and_then(|body| {
                // Quick and dirty way to avoid having to write a custom deserializer
                let body = Regex::new(r#""board":"3""#)
                    .unwrap()
                    .replace(str::from_utf8(&body)?, r#""board":"_3""#);
                let BoardsWrapper { boards } = serde_json::from_str(&body)?;
                Ok(boards)
            }),
    );
    runtime.shutdown_now().wait().unwrap();

    for Board {
        board,
        per_page,
        pages,
        is_archived,
    } in boards.unwrap()
    {
        assert_eq!(
            board.is_archived(),
            is_archived,
            "/{}/'s correct archive status is {}",
            board,
            is_archived,
        );
        assert_eq!(
            board.max_threads(),
            per_page * pages,
            "/{}/'s correct max thread count is {}",
            board,
            per_page * pages,
        );
    }
}

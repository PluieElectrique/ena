//! Asagi schema notes (the following only applies to scraped posts):
//!   * `media_id` is set by triggers
//!   * `poster_ip` and `subnum` (used for ghost posts?) are always 0
//!   * `email` and `delpass` are always `NULL`
//!   * Timestamps are "adjusted" to America/New_York if the `adjust_timestamps` settings is on
//!   * Asagi scrapes `exif`, but we currently don't since it doesn't seem necessary

use actix::prelude::*;
use chrono::prelude::*;
use chrono_tz::America;
use futures::prelude::*;
use my::prelude::*;
use my::{self, Pool};
use tokio::runtime::Runtime;

use four_chan::{Board, OpData, Post};
use html;

const BOARD_REPLACE: &str = "%%BOARD%%";
const CHARSET_REPLACE: &str = "%%CHARSET%%";
const BOARD_SQL: &str = include_str!("../sql/boards.sql");
const INDEX_COUNTERS_SQL: &str = include_str!("../sql/index_counters.sql");
const TRIGGER_SQL: &str = include_str!("../sql/triggers.sql");

const INSERT_QUERY: &str = "INSERT INTO `%%BOARD%%` (num, subnum, thread_num, op, timestamp,
timestamp_expired, preview_orig, preview_w, preview_h, media_filename, media_w, media_h, media_size,
media_hash, media_orig, spoiler, capcode, name, trip, title, comment, sticky, locked, poster_hash,
poster_country)
VALUES (:num, :subnum, :thread_num, :op, :timestamp, :timestamp_expired, :preview_orig, :preview_w,
:preview_h, :media_filename, :media_w, :media_h, :media_size, :media_hash, :media_orig, :spoiler,
:capcode, :name, :trip, :title, :comment, :sticky, :locked, :poster_hash, :poster_country)";

const NEW_MEDIA_QUERY: &str = "SELECT
    IF(total = 1, media_orig, NULL),
    preview_orig
FROM `%%BOARD%%`
INNER JOIN `%%BOARD%%_images` ON
    `%%BOARD%%`.media_id = `%%BOARD%%_images`.media_id
    AND preview_orig in (preview_reply, preview_op)
WHERE doc_id BETWEEN
    LAST_INSERT_ID()
    AND LAST_INSERT_ID() + ROW_COUNT() - 1;";

const UPDATE_OP_QUERY: &str = "UPDATE `%%BOARD%%`
SET sticky = :sticky, locked = :locked, timestamp_expired = :timestamp_expired
WHERE num = :num AND subnum = 0";

const UPDATE_COMMENT_QUERY: &str = "UPDATE `%%BOARD%%`
SET comment = :comment
WHERE num = :num AND subnum = 0";

const MARK_REMOVED_QUERY: &str = "UPDATE `%%BOARD%%`
SET deleted = :deleted, timestamp_expired = :timestamp_expired
WHERE num = :num AND subnum = 0";

pub struct Database {
    pool: Pool,
    adjust_timestamps: bool,
}

impl Database {
    pub fn new(
        pool: Pool,
        boards: &[Board],
        charset: &str,
        adjust_timestamps: bool,
        create_index_counters: bool,
    ) -> Result<Self, my::errors::Error> {
        let charset_board_sql = BOARD_SQL.replace(CHARSET_REPLACE, charset);

        let mut init_sql = String::new();
        for board in boards {
            init_sql.push_str(&charset_board_sql.replace(BOARD_REPLACE, &board.to_string()));
            init_sql.push_str(&TRIGGER_SQL.replace(BOARD_REPLACE, &board.to_string()));
        }

        let mut runtime = Runtime::new().unwrap();
        if create_index_counters {
            runtime.block_on(
                pool.get_conn()
                    .and_then(|conn| conn.drop_query(init_sql))
                    .and_then(|conn| conn.drop_query(INDEX_COUNTERS_SQL))
                    // If we don't disconnect the runtime won't shutdown
                    .and_then(|conn| conn.disconnect()),
            )?;
        } else {
            runtime.block_on(
                pool.get_conn()
                    .and_then(|conn| conn.drop_query(init_sql))
                    // If we don't disconnect the runtime won't shutdown
                    .and_then(|conn| conn.disconnect()),
            )?;
        }
        runtime.shutdown_on_idle().wait().unwrap();

        Ok(Self {
            pool,
            adjust_timestamps,
        })
    }

    fn adjust_timestamp(&self, utc_timestamp: u64) -> u64 {
        if self.adjust_timestamps {
            America::New_York
                .timestamp(utc_timestamp as i64, 0)
                .naive_local()
                .timestamp() as u64
        } else {
            utc_timestamp
        }
    }
}

impl Actor for Database {
    type Context = Context<Self>;
}

pub struct InsertPosts(pub Board, pub Vec<Post>);
impl Message for InsertPosts {
    type Result = Result<Vec<String>, my::errors::Error>;
}

impl Handler<InsertPosts> for Database {
    type Result = ResponseFuture<Vec<String>, my::errors::Error>;

    fn handle(&mut self, msg: InsertPosts, _ctx: &mut Self::Context) -> Self::Result {
        let params: Vec<_> = msg.1.into_iter().map(|post| {
            let mut params = params! {
                "num" => post.no,
                "subnum" => 0,
                "thread_num" => if post.reply_to == 0 {
                    post.no
                } else {
                    post.reply_to
                },
                "op" => post.reply_to == 0,
                "timestamp" => self.adjust_timestamp(post.time),
                "timestamp_expired" => post.op_data.archived_on.map_or(0, |t| self.adjust_timestamp(t)),
                "capcode" => {
                    post.capcode.map_or(String::from("N"), |mut capcode| {
                        if capcode == "manager" {
                            String::from("G")
                        } else {
                            capcode.truncate(1);
                            capcode.make_ascii_uppercase();
                            capcode
                        }
                    })
                },
                "name" => post.name.map(|name| html::unescape(&name)),
                "trip" => post.trip,
                "title" => post.subject.map(|subject| html::unescape(&subject)),
                "comment" => post.comment.map(|comment| html::clean(&comment).unwrap()),
                "sticky" => post.op_data.sticky,
                // We only want to mark threads as locked if they are closed before being archived.
                // This is because all archived threads are marked as closed.
                "locked" => post.op_data.closed && !post.op_data.archived,
                "poster_hash" => post.id,
                "poster_country" => post.country,
            };

            let mut image_params;
            if let Some(image) = post.image {
                image_params = params! {
                    "media_filename" => image.filename + &image.ext,
                    "media_orig" => format!("{}{}", image.time_millis, image.ext),
                    "media_w" => image.image_width,
                    "media_h" => image.image_height,
                    "media_size" => image.filesize,
                    "media_hash" => image.md5,
                    "preview_orig" => format!("{}s.jpg", image.time_millis),
                    "preview_w" => image.thumbnail_width,
                    "preview_h" => image.thumbnail_height,
                    "spoiler" => image.spoiler,
                };
            } else {
                image_params = params! {
                    "media_filename" => None::<String>,
                    "media_orig" => None::<String>,
                    "media_w" => 0,
                    "media_h" => 0,
                    "media_size" => 0,
                    "media_hash" => None::<String>,
                    "preview_orig" => None::<String>,
                    "preview_w" => 0,
                    "preview_h" => 0,
                    "spoiler" => false,
                };
            }
            params.append(&mut image_params);

            params
        }).collect();

        let insert_query = INSERT_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        let new_media_query = NEW_MEDIA_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.batch_exec(insert_query, params))
                .and_then(|conn| conn.query(new_media_query))
                .and_then(|results| {
                    results.reduce_and_drop(vec![], |mut files: Vec<String>, row| {
                        let (media, preview) = my::from_row(row);
                        if let Some(media) = media {
                            files.push(media);
                        }
                        files.push(preview);
                        files
                    })
                }).map(|(_conn, files)| files),
        )
    }
}

pub struct UpdateOp(pub Board, pub u64, pub OpData);
impl Message for UpdateOp {
    type Result = Result<(), my::errors::Error>;
}

impl Handler<UpdateOp> for Database {
    type Result = ResponseFuture<(), my::errors::Error>;

    fn handle(&mut self, msg: UpdateOp, _ctx: &mut Self::Context) -> Self::Result {
        let params = params! {
            "num" => msg.1,
            "sticky" => msg.2.sticky,
            "locked" => msg.2.closed && !msg.2.archived,
            "timestamp_expired" => msg.2.archived_on.map_or(0, |t| self.adjust_timestamp(t)),
        };
        let update_op_query = UPDATE_OP_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.drop_exec(update_op_query, params))
                .map(|_conn| ()),
        )
    }
}

pub struct UpdateComment(pub Board, pub Vec<(u64, Option<String>)>);
impl Message for UpdateComment {
    type Result = Result<(), my::errors::Error>;
}

impl Handler<UpdateComment> for Database {
    type Result = ResponseFuture<(), my::errors::Error>;

    fn handle(&mut self, msg: UpdateComment, _ctx: &mut Self::Context) -> Self::Result {
        let params = msg.1.into_iter().map(move |(no, comment)| {
            params! {
                "num" => no,
                comment,
            }
        });
        let update_comment_query = UPDATE_COMMENT_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.batch_exec(update_comment_query, params))
                .map(|_conn| ()),
        )
    }
}

pub enum RemovedStatus {
    Archived,
    Deleted,
}

pub struct MarkPostsRemoved(pub Board, pub Vec<(u64, RemovedStatus)>, pub DateTime<Utc>);
impl Message for MarkPostsRemoved {
    type Result = Result<(), my::errors::Error>;
}

impl Handler<MarkPostsRemoved> for Database {
    type Result = ResponseFuture<(), my::errors::Error>;

    fn handle(&mut self, msg: MarkPostsRemoved, _ctx: &mut Self::Context) -> Self::Result {
        let timestamp_expired = self.adjust_timestamp(msg.2.timestamp() as u64);
        let params = msg.1.into_iter().map(move |(no, status)| {
            params! {
                "num" => no,
                "deleted" => match status {
                    RemovedStatus::Archived => false,
                    RemovedStatus::Deleted => true,
                },
                timestamp_expired,
            }
        });
        let mark_removed_query = MARK_REMOVED_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.batch_exec(mark_removed_query, params))
                .map(|_conn| ()),
        )
    }
}

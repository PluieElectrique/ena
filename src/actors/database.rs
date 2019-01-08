use std::{collections::HashMap, sync::Arc};

use actix::prelude::*;
use chrono::prelude::*;
use chrono_tz::America;
use futures::{future, prelude::*};
use mysql_async::{error::Error, params, prelude::*, Opts, Pool, Value};
use tokio::runtime::Runtime;

use crate::{
    config::{Config, ScrapingConfig},
    four_chan::{Board, OpData, Post},
    html,
};

const DATABASE_MAILBOX_CAPACITY: usize = 1000;

const BOARD_REPLACE: &str = "%%BOARD%%";
const CHARSET_REPLACE: &str = "%%CHARSET%%";

/// An actor which provides an interface to the MySQL database.
pub struct Database {
    boards: Arc<HashMap<Board, ScrapingConfig>>,
    pool: Pool,
    adjust_timestamps: bool,
}

impl Database {
    pub fn try_new(config: &Config) -> Result<Self, Error> {
        let pool = Pool::from_url(&config.database_media.database_url)?;
        let mut runtime = Runtime::new().unwrap();

        if config.asagi_compat.create_index_counters {
            runtime.block_on(
                pool.get_conn()
                    .and_then(|conn| conn.drop_query(include_str!("../sql/index_counters.sql")))
                    .and_then(|conn| conn.disconnect()),
            )?;
        }

        info!("Creating database tables and triggers");
        runtime.block_on({
            let boards: Vec<Board> = config.boards.keys().cloned().collect();
            let pool = pool.clone();
            let board_sql = include_str!("../sql/boards.sql")
                .replace(CHARSET_REPLACE, &config.database_media.charset);
            future::join_all(boards.into_iter().map(move |board| {
                let mut init_sql = String::new();
                init_sql.push_str(&board_replace(board, &board_sql));
                init_sql.push_str(&board_replace(board, include_str!("../sql/triggers.sql")));

                pool.get_conn()
                    .and_then(|conn| conn.drop_query(init_sql))
                    // If we don't disconnect these connections, and try to use them on the Actix
                    // current_thread runtime after we shutdown this runtime, we will get a "reactor
                    // gone" message.
                    .and_then(|conn| conn.disconnect())
                    .map(move |_| debug!("/{}/: Created table and triggers", board))
            }))
        })?;
        runtime.shutdown_on_idle().wait().unwrap();

        Ok(Self {
            boards: config.boards.clone(),
            pool,
            adjust_timestamps: config.asagi_compat.adjust_timestamps,
        })
    }
}

impl Actor for Database {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(DATABASE_MAILBOX_CAPACITY);
    }
}

pub struct GetUnarchivedThreads(pub Board, pub Vec<u64>);
impl Message for GetUnarchivedThreads {
    type Result = Result<Vec<u64>, Error>;
}

impl Handler<GetUnarchivedThreads> for Database {
    type Result = ResponseFuture<Vec<u64>, Error>;

    fn handle(&mut self, msg: GetUnarchivedThreads, _ctx: &mut Self::Context) -> Self::Result {
        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| {
                    conn.drop_query("CREATE TEMPORARY TABLE archive_threads (id int unsigned);")
                })
                .and_then({
                    let params = msg.1.into_iter().map(|id| params! { id });
                    |conn| conn.batch_exec("INSERT INTO archive_threads SET id = :id;", params)
                })
                .and_then({
                    let query = board_replace(
                        msg.0,
                        "DELETE archive_threads FROM archive_threads \
                         INNER JOIN `%%BOARD%%` ON id = num AND subnum = 0 \
                         WHERE timestamp_expired != 0; \
                         DELETE archive_threads FROM archive_threads \
                         INNER JOIN `%%BOARD%%_deleted` ON id = num AND subnum = 0;",
                    );
                    |conn| conn.drop_query(query)
                })
                .and_then(|conn| conn.query("SELECT id FROM archive_threads;"))
                .and_then(|result| result.collect_and_drop())
                .and_then(|(conn, nums)| {
                    // It seems the table persists and causes errors if the connection is reused, so
                    // we drop it explicitly
                    conn.drop_query("DROP TABLE archive_threads;")
                        .map(|_conn| nums)
                }),
        )
    }
}

pub struct InsertPosts(pub Board, pub u64, pub Vec<Post>);
impl Message for InsertPosts {
    type Result = Result<Vec<String>, Error>;
}

impl Handler<InsertPosts> for Database {
    type Result = ResponseFuture<Vec<String>, Error>;

    fn handle(&mut self, msg: InsertPosts, _ctx: &mut Self::Context) -> Self::Result {
        assert!(!msg.2.is_empty(), "Cannot insert empty thread");

        let board = msg.0;
        let num_start = msg.2[0].no;
        let num_end = msg.2.last().unwrap().no;
        let adjust_timestamps = self.adjust_timestamps;
        let params = msg.2.into_iter().map(move |post| {
            let no = post.no;
            let mut params = params! {
                "num" => post.no,
                // subnum is used for ghost posts. All scraped posts have a subnum of 0.
                "subnum" => 0,
                "thread_num" => if post.reply_to == 0 {
                    post.no
                } else {
                    post.reply_to
                },
                "op" => post.reply_to == 0,
                "timestamp" => post.time.adjust(adjust_timestamps),
                "timestamp_expired" => post.op_data.archived_on.map_or(
                    0, |t| t.adjust(adjust_timestamps)
                ),
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
                "name" => post.name.map(|name| html::unescape(name, Some((board, no)))),
                "trip" => post.trip,
                "title" => post.subject.map(|subject| html::unescape(subject, Some((board, no)))),
                "comment" => post.comment.map(|comment| html::clean(comment, Some((board, no)))),
                "sticky" => post.op_data.sticky,
                // We only want to mark threads as locked if they are closed before being archived.
                // This is because all archived threads are marked as closed.
                "locked" => post.op_data.closed && !post.op_data.archived,
                "poster_hash" => post.id.map(|id| if id == "Developer" {
                    String::from("Dev")
                } else {
                    id
                }),
                // NOTE: Asagi ignores the "XX" and "A1" flags, but why? Should we? For what it's
                // worth, they aren't in boards.json.
                "poster_country" => post.country,
            };

            let mut image_params = if let Some(image) = post.image {
                params! {
                    "media_filename" => image.filename + &image.ext,
                    "media_orig" => format!("{}{}", image.time_millis, image.ext),
                    "media_w" => image.image_width,
                    "media_h" => image.image_height,
                    "media_size" => image.filesize,
                    "media_hash" => image.md5,
                    "preview_orig" => if image.thumbnail_width == 0 && image.thumbnail_height == 0 {
                        None
                    } else {
                        Some(format!("{}s.jpg", image.time_millis))
                    },
                    "preview_w" => image.thumbnail_width,
                    "preview_h" => image.thumbnail_height,
                    "spoiler" => image.spoiler,
                }
            } else {
                params! {
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
                }
            };
            params.append(&mut image_params);

            params
        });

        // Columns missing from this query like media_id, poster_ip, email, delpass, and exif are
        // either always set to their defaults, set by triggers, or unused by Ena
        let insert_query = board_replace(
            msg.0,
            "INSERT INTO `%%BOARD%%` (num, subnum, thread_num, op, timestamp, timestamp_expired, \
             preview_orig, preview_w, preview_h, media_filename, media_w, media_h, media_size, \
             media_hash, media_orig, spoiler, capcode, name, trip, title, comment, sticky, locked, \
             poster_hash, poster_country) \
             SELECT :num, :subnum, :thread_num, :op, :timestamp, :timestamp_expired, :preview_orig, \
             :preview_w, :preview_h, :media_filename, :media_w, :media_h, :media_size, :media_hash, \
             :media_orig, :spoiler, :capcode, :name, :trip, :title, :comment, :sticky, :locked, \
             :poster_hash, :poster_country \
             WHERE NOT EXISTS ( \
                 SELECT * FROM `%%BOARD%%_deleted` WHERE num in (:num, :thread_num) AND subnum = 0) \
             ON DUPLICATE KEY UPDATE \
                 sticky = VALUES(sticky), \
                 locked = VALUES(locked), \
                 timestamp_expired = VALUES(timestamp_expired), \
                 comment = VALUES(comment), \
                 spoiler = VALUES(spoiler);",
        );

        let download_media = self.boards[&board].download_media;
        let download_thumbs = self.boards[&board].download_thumbs;
        if !download_media && !download_thumbs {
            Box::new(
                self.pool
                    .get_conn()
                    .and_then(|conn| conn.batch_exec(insert_query, params))
                    .map(|_conn| vec![]),
            )
        } else {
            let thread_num = msg.1;
            Box::new(
                self.pool
                    .get_conn()
                    .and_then({
                        let query = board_replace(
                            msg.0,
                            "SELECT COALESCE(MAX(num) + 1, :num_start) \
                             FROM `%%BOARD%%` \
                             WHERE
                                 num BETWEEN :num_start AND :num_end \
                                 AND subnum = 0 \
                                 AND thread_num = :thread_num;",
                        );
                        move |conn| {
                            conn.first_exec(query, params! { num_start, num_end, thread_num })
                        }
                    })
                    .and_then({
                        let new_media_query = board_replace(
                            msg.0,
                            "SELECT
                                 IF(media_orig = media, media_orig, NULL), \
                                 preview_orig \
                             FROM `%%BOARD%%` \
                             INNER JOIN `%%BOARD%%_images` ON
                                 `%%BOARD%%`.media_id = `%%BOARD%%_images`.media_id \
                                 AND preview_orig IN (preview_reply, preview_op) \
                             WHERE
                                 num BETWEEN :num_start AND :num_end \
                                 AND subnum = 0 \
                                 AND thread_num = :thread_num \
                                 AND banned = 0;",
                        );

                        move |(conn, next_num): (_, Option<(u64,)>)| {
                            conn.batch_exec(insert_query, params).and_then(move |conn| {
                                conn.prep_exec(
                                    new_media_query,
                                    params! {
                                        "num_start" => next_num.unwrap().0,
                                        num_end,
                                        thread_num,
                                    },
                                )
                            })
                        }
                    })
                    .and_then(move |results| {
                        results.reduce_and_drop(vec![], move |mut files: Vec<String>, row| {
                            let (media, preview) = mysql_async::from_row(row);
                            if download_media {
                                if let Some(media) = media {
                                    files.push(media);
                                }
                            }
                            if download_thumbs {
                                if let Some(preview) = preview {
                                    files.push(preview);
                                }
                            }
                            files
                        })
                    })
                    .map(|(_conn, files)| files),
            )
        }
    }
}

pub struct UpdateOp(pub Board, pub u64, pub OpData);
impl Message for UpdateOp {
    type Result = Result<(), Error>;
}

impl Handler<UpdateOp> for Database {
    type Result = ResponseFuture<(), Error>;

    fn handle(&mut self, msg: UpdateOp, _ctx: &mut Self::Context) -> Self::Result {
        let mut params = params! {
            "num" => msg.1,
            "sticky" => msg.2.sticky,
            "timestamp_expired" => msg.2.archived_on.map_or(0, |t| t.adjust(self.adjust_timestamps)),
        };

        // Preserve the locked status of a thread by only updating it if it hasn't been archived yet
        let query;
        if msg.2.archived {
            query = board_replace(
                msg.0,
                "UPDATE `%%BOARD%%` \
                 SET sticky = :sticky, timestamp_expired = :timestamp_expired \
                 WHERE num = :num AND subnum = 0",
            );
        } else {
            query = board_replace(
                msg.0,
                "UPDATE `%%BOARD%%` \
                 SET sticky = :sticky, locked = :locked, timestamp_expired = :timestamp_expired \
                 WHERE num = :num AND subnum = 0",
            );
            params.push((String::from("locked"), Value::from(msg.2.closed)));
        }

        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.drop_exec(query, params))
                .map(|_conn| ()),
        )
    }
}

pub struct UpdatePost(pub Board, pub Vec<(u64, Option<String>, Option<bool>)>);
impl Message for UpdatePost {
    type Result = Result<(), Error>;
}

impl Handler<UpdatePost> for Database {
    type Result = ResponseFuture<(), Error>;

    fn handle(&mut self, msg: UpdatePost, _ctx: &mut Self::Context) -> Self::Result {
        let board = msg.0;
        let query = board_replace(
            board,
            "UPDATE `%%BOARD%%` \
             SET comment = :comment, spoiler = :spoiler \
             WHERE num = :num AND subnum = 0",
        );
        let params = msg.1.into_iter().map(move |(no, comment, spoiler)| {
            params! {
                "num" => no,
                "comment" => comment.map(|comment| html::clean(comment, Some((board, no)))),
                "spoiler" => spoiler.unwrap_or(false),
            }
        });
        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.batch_exec(query, params))
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
    type Result = Result<(), Error>;
}

impl Handler<MarkPostsRemoved> for Database {
    type Result = ResponseFuture<(), Error>;

    fn handle(&mut self, msg: MarkPostsRemoved, _ctx: &mut Self::Context) -> Self::Result {
        let query = board_replace(
            msg.0,
            "UPDATE `%%BOARD%%` \
             SET deleted = :deleted, timestamp_expired = :timestamp_expired \
             WHERE num = :num AND subnum = 0",
        );
        let timestamp_expired = msg.2.adjust(self.adjust_timestamps);
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
        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.batch_exec(query, params))
                .map(|_conn| ()),
        )
    }
}

trait TimestampExt {
    fn adjust(&self, adjust: bool) -> u64;
}

impl TimestampExt for u64 {
    fn adjust(&self, adjust: bool) -> u64 {
        if adjust {
            America::New_York
                .timestamp(*self as i64, 0)
                .naive_local()
                .timestamp() as u64
        } else {
            *self
        }
    }
}

impl TimestampExt for DateTime<Utc> {
    fn adjust(&self, adjust: bool) -> u64 {
        if adjust {
            self.with_timezone(&America::New_York)
                .naive_local()
                .timestamp() as u64
        } else {
            self.timestamp() as u64
        }
    }
}

fn board_replace(board: Board, query: &str) -> String {
    query.replace(BOARD_REPLACE, &board.to_string())
}

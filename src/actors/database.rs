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
use my::{self, Pool, Value};
use tokio::runtime::Runtime;

use four_chan::{Board, OpData, Post};
use html;

const BOARD_REPLACE: &str = "%%BOARD%%";
const CHARSET_REPLACE: &str = "%%CHARSET%%";
const BOARD_SQL: &str = include_str!("../sql/boards.sql");
const INDEX_COUNTERS_SQL: &str = include_str!("../sql/index_counters.sql");
const TRIGGER_SQL: &str = include_str!("../sql/triggers.sql");

// thread_nums is a temporary table created in Handler<GetUnarchivedThreads>
const UNARCHIVED_THREAD_QUERY: &str = "
SELECT thread_nums.num
FROM thread_nums
INNER JOIN `%%BOARD%%` ON `%%BOARD%%`.num = thread_nums.num
WHERE
    timestamp_expired = 0
    AND deleted = 0
    AND subnum = 0;";

const NEXT_NUM_QUERY: &str = "
SELECT COALESCE(MAX(num) + 1, :num_start)
FROM `%%BOARD%%`
WHERE
    num BETWEEN :num_start AND :num_end
    AND subnum = 0
    AND thread_num = :thread_num;";

const INSERT_QUERY: &str = "
INSERT INTO `%%BOARD%%` (num, subnum, thread_num, op, timestamp, timestamp_expired, preview_orig,
preview_w, preview_h, media_filename, media_w, media_h, media_size, media_hash, media_orig, spoiler,
capcode, name, trip, title, comment, sticky, locked, poster_hash, poster_country)
VALUES (:num, :subnum, :thread_num, :op, :timestamp, :timestamp_expired, :preview_orig, :preview_w,
:preview_h, :media_filename, :media_w, :media_h, :media_size, :media_hash, :media_orig, :spoiler,
:capcode, :name, :trip, :title, :comment, :sticky, :locked, :poster_hash, :poster_country)
ON DUPLICATE KEY UPDATE
    sticky = VALUES(sticky),
    locked = VALUES(locked),
    timestamp_expired = VALUES(timestamp_expired),
    comment = VALUES(comment),
    spoiler = VALUES(spoiler);";

const NEW_MEDIA_QUERY: &str = "
SELECT
    IF(media_orig = media, media_orig, NULL),
    preview_orig
FROM `%%BOARD%%`
INNER JOIN `%%BOARD%%_images` ON
    `%%BOARD%%`.media_id = `%%BOARD%%_images`.media_id
    AND preview_orig in (preview_reply, preview_op)
WHERE
    num BETWEEN :num_start AND :num_end
    AND subnum = 0
    AND thread_num = :thread_num
    AND banned = 0;";

const UPDATE_OP_QUERY: &str = "
UPDATE `%%BOARD%%`
SET sticky = :sticky, locked = :locked, timestamp_expired = :timestamp_expired
WHERE num = :num AND subnum = 0";

const UPDATE_OP_NO_LOCK_QUERY: &str = "
UPDATE `%%BOARD%%`
SET sticky = :sticky, timestamp_expired = :timestamp_expired
WHERE num = :num AND subnum = 0";

const UPDATE_POST_QUERY: &str = "
UPDATE `%%BOARD%%`
SET comment = :comment, spoiler = :spoiler
WHERE num = :num AND subnum = 0";

const MARK_REMOVED_QUERY: &str = "
UPDATE `%%BOARD%%`
SET deleted = :deleted, timestamp_expired = :timestamp_expired
WHERE num = :num AND subnum = 0";

pub struct Database {
    pool: Pool,
    adjust_timestamps: bool,
    download_media: bool,
    download_thumbs: bool,
}

impl Database {
    pub fn new(
        pool: Pool,
        boards: &[Board],
        charset: &str,
        adjust_timestamps: bool,
        create_index_counters: bool,
        download_media: bool,
        download_thumbs: bool,
    ) -> Result<Self, my::errors::Error> {
        let charset_board_sql = BOARD_SQL.replace(CHARSET_REPLACE, charset);

        let mut init_sql = String::new();
        for board in boards {
            init_sql.push_str(&charset_board_sql.replace(BOARD_REPLACE, &board.to_string()));
            init_sql.push_str(&TRIGGER_SQL.replace(BOARD_REPLACE, &board.to_string()));
        }

        if create_index_counters {
            init_sql.push_str(INDEX_COUNTERS_SQL);
        }

        let mut runtime = Runtime::new().unwrap();
        runtime.block_on(
            pool.get_conn()
                .and_then(|conn| conn.drop_query(init_sql))
                .and_then(|conn| conn.disconnect()),
        )?;
        runtime.shutdown_on_idle().wait().unwrap();

        Ok(Self {
            pool,
            adjust_timestamps,
            download_media,
            download_thumbs,
        })
    }
}

impl Actor for Database {
    type Context = Context<Self>;
}

pub struct GetUnarchivedThreads(pub Board, pub Vec<u64>);
impl Message for GetUnarchivedThreads {
    type Result = Result<Vec<u64>, my::errors::Error>;
}

impl Handler<GetUnarchivedThreads> for Database {
    type Result = ResponseFuture<Vec<u64>, my::errors::Error>;

    fn handle(&mut self, msg: GetUnarchivedThreads, _ctx: &mut Self::Context) -> Self::Result {
        let params = msg.1.into_iter().map(|num| {
            params! {
                num,
            }
        });

        let thread_query = UNARCHIVED_THREAD_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());

        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| {
                    conn.drop_query("CREATE TEMPORARY TABLE thread_nums (num int unsigned);")
                }).and_then(|conn| conn.batch_exec("INSERT INTO thread_nums SET num = :num;", params))
                .and_then(|conn| conn.query(thread_query))
                .and_then(|result| result.collect_and_drop())
                .map(|(_conn, nums)| nums),
        )
    }
}

pub struct InsertPosts(pub Board, pub u64, pub Vec<Post>);
impl Message for InsertPosts {
    type Result = Result<Vec<String>, my::errors::Error>;
}

impl Handler<InsertPosts> for Database {
    type Result = ResponseFuture<Vec<String>, my::errors::Error>;

    fn handle(&mut self, msg: InsertPosts, _ctx: &mut Self::Context) -> Self::Result {
        let num_start = msg.2[0].no;
        let num_end = msg.2[msg.2.len() - 1].no;
        let adjust_timestamps = self.adjust_timestamps;
        let params = msg.2.into_iter().map(move |post| {
            let mut params = params! {
                "num" => post.no,
                "subnum" => 0,
                "thread_num" => if post.reply_to == 0 {
                    post.no
                } else {
                    post.reply_to
                },
                "op" => post.reply_to == 0,
                "timestamp" => post.time.adjust(adjust_timestamps),
                "timestamp_expired" => post.op_data.archived_on.map_or(0, |t| t.adjust(adjust_timestamps)),
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

            let mut image_params = if let Some(image) = post.image {
                params! {
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

        let next_num_query = NEXT_NUM_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        let insert_query = INSERT_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        let new_media_query = NEW_MEDIA_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());

        if !self.download_media && !self.download_thumbs {
            Box::new(
                self.pool
                    .get_conn()
                    .and_then(|conn| conn.batch_exec(insert_query, params))
                    .map(|_conn| vec![]),
            )
        } else {
            let thread_num = msg.1;
            let download_media = self.download_media;
            let download_thumbs = self.download_thumbs;
            Box::new(
                self.pool
                    .get_conn()
                    .and_then(move |conn| {
                        conn.first_exec(
                            next_num_query,
                            params! {
                                num_start,
                                num_end,
                                thread_num,
                            },
                        )
                    }).and_then(move |(conn, next_num): (_, Option<(u64,)>)| {
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
                    }).and_then(move |results| {
                        results.reduce_and_drop(vec![], move |mut files: Vec<String>, row| {
                            let (media, preview) = my::from_row(row);
                            if download_media {
                                if let Some(media) = media {
                                    files.push(media);
                                }
                            }
                            if download_thumbs {
                                files.push(preview);
                            }
                            files
                        })
                    }).map(|(_conn, files)| files),
            )
        }
    }
}

pub struct UpdateOp(pub Board, pub u64, pub OpData);
impl Message for UpdateOp {
    type Result = Result<(), my::errors::Error>;
}

impl Handler<UpdateOp> for Database {
    type Result = ResponseFuture<(), my::errors::Error>;

    fn handle(&mut self, msg: UpdateOp, _ctx: &mut Self::Context) -> Self::Result {
        let mut params = params! {
            "num" => msg.1,
            "sticky" => msg.2.sticky,
            "timestamp_expired" => msg.2.archived_on.map_or(0, |t| t.adjust(self.adjust_timestamps)),
        };

        // Preserve the locked status of a thread by only updating it if it hasn't been archived yet
        let update_op_query;
        if msg.2.archived {
            update_op_query = UPDATE_OP_NO_LOCK_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        } else {
            update_op_query = UPDATE_OP_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
            params.push((String::from("locked"), Value::from(msg.2.closed)));
        }

        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.drop_exec(update_op_query, params))
                .map(|_conn| ()),
        )
    }
}

pub struct UpdatePost(pub Board, pub Vec<(u64, Option<String>, Option<bool>)>);
impl Message for UpdatePost {
    type Result = Result<(), my::errors::Error>;
}

impl Handler<UpdatePost> for Database {
    type Result = ResponseFuture<(), my::errors::Error>;

    fn handle(&mut self, msg: UpdatePost, _ctx: &mut Self::Context) -> Self::Result {
        let params = msg.1.into_iter().map(move |(no, comment, spoiler)| {
            params! {
                "num" => no,
                comment,
                "spoiler" => spoiler.unwrap_or(false),
            }
        });
        let update_post_query = UPDATE_POST_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.batch_exec(update_post_query, params))
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
        let mark_removed_query = MARK_REMOVED_QUERY.replace(BOARD_REPLACE, &msg.0.to_string());
        Box::new(
            self.pool
                .get_conn()
                .and_then(|conn| conn.batch_exec(mark_removed_query, params))
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

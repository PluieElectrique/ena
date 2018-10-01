# Ena

A 4chan scraper. Currently designed to be a (mostly) compatible replacement for Asagi.

## Getting started

Install and configure a MySQL-compatible database (tested on MariaDB 10.1). If you are running Ena with FoolFuuka, consider referring to the [instructions](https://wiki.bibanon.org/FoolFuuka) at the Bibliotheca Anonoma wiki.

Copy the default configuration file `ena.example.toml` to `ena.toml` and adjust the settings as necessary. Then, [install Rust](https://www.rust-lang.org/install.html) and compile and run Ena with:

```sh
cargo run --release
```

## Logging

By default, only errors are logged. Logging is configured by setting the `RUST_LOG` environment variable. For example, to turn on all logging, use `RUST_LOG=ena`. Or, to just show warnings and errors, use `RUST_LOG=ena=warn`. See the `env_logger` [documentation](https://docs.rs/env_logger/*/env_logger/) for more information.

## Differences from Asagi

[desuarchive's fork](https://github.com/desuarchive/asagi) is used as the reference for these comparisons.

### Scraping mechanics

* Existing posts in modified threads are only updated when the OP data, comment, or spoiler flag changes
* On start, all live threads are fetched and updated, regardless of whether they've changed or not
* On start, all archived threads are fetched and updated if they are not marked as archived in the database
* Closed threads remain locked even after they are archived (In Asagi, closed threads are unlocked on the refetch after archival)
* The `exif` column (a JSON blob of exif data, unique IPs, `since4pass`, and troll countries) is not used
* The old media/thumbs directory structure is not supported
* The "anchor thread" heuristic is used instead of the "page theshold" heuristic for determining when a thread was bumped off and when it was deleted
* In ambiguous cases, removed threads are assumed to be bumped off and not deleted
* When possible, the `timestamp_expired` for a deleted thread or post is taken from the `Last-Modified` header of the request, and not the time at which it was processed

### Post/media processing

* More tags (like `[sjis]` and `[qstcolor]`) are supported
* All HTML is serialized as fragments according to the [HTML spec](https://html.spec.whatwg.org/multipage/parsing.html#serialising-html-fragments). This leads to more escapes (e.g. Ena produces `&gt;&gt;12345` whereas Asagi produces `>>12345`)
* The `XX` and `A1` country flags are not ignored
* A fixed set of HTML character references are replaced in usernames and titles (In addition to the references Ena replaces, Asagi also replaces all numeric character references of the form `&#\d+;`)
* Unknown HTML tags may have their attributes reordered
* Posts are trimmed of trailing whitespace (Asagi trims whitespace from the start and end of each line). This may cause blankposts to become empty, non-NULL strings
* Setting the group file permission (`webserverGroup`) of downloaded media is currently not supported
* Media are only downloaded the first time they or the post they are in is seen. This means that if a thread is inserted and its media are queued to download, but the program crashes, on restart those media that didn't download will **never** be downloaded.

### Database

* If the OP post of a thread is moved to the `%%BOARD%%_deleted` table, no new posts from that thread will be inserted
* If a live thread is moved to the `%%BOARD%%_deleted` while Ena is running, Ena will continue to monitor it and produce errors while trying to update it. However, no data will actually be written
* `media_filename` is not updated when existing posts are updated
* PostgreSQL is not supported
* The `%%BOARD%%_daily` and `%%BOARD%%_users` tables are not created

## Known defects

Ena strives to be an accurate scraper, but it isn't perfect.

### Scraping mechanics

* Requests are not retried. If a media request fails, the media will never be downloaded. If a thread request fails, data will be lost unless the thread is fetched again in the future and the request succeeds
* Though the bumped off/deleted detection should be better than Asagi, it still has flaws: (If absolute accuracy is required a `HEAD` request could be sent for each thread)
    * If `poll_interval` is too long or the scraped board moves too quickly, threads may not be marked correctly
    * If the last _n_ threads from the catalog are deleted, they will be marked as bumped off

### Data loss

* Threads and posts deleted while Ena is stopped will not be marked as such when it restarts
* If Ena crashes in the process of updating an archived thread, on restart the thread may be marked as "archived" even if the update never happened. Thus, changes between the last poll of the thread and the archival of it may be lost
* As mentioned above, if Ena crashes while media are queued to download, on restart they will not be re-queued. Thus, they will never be downloaded

## Legal

This program is licensed under the AGPLv3. See the `LICENSE` file for more information.

This program contains code from:

* [Asagi](https://github.com/desuarchive/asagi) (GPLv3)
* [futures-rs](https://github.com/rust-lang-nursery/futures-rs) (MIT)
* [html5ever](https://github.com/servo/html5ever) (MIT)

See the `NOTICE` file for more information.

This program is completely unofficial and not affiliated with 4chan in any way.

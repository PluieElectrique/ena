# Ena

![minimum Rust version: 1.32](https://img.shields.io/badge/minimum%20Rust%20version-1.32-brightgreen.svg)

A 4chan scraper. Currently designed to be a (mostly) compatible, improved replacement for Asagi.

## Features

* Low memory usage (maybe around 100–200 MB to scrape all of 4chan)
* Asagi compatibility
* Rate limiting
* Per-board scraping configuration
* Request retrying

## Getting started

Install and configure a MySQL-compatible database (tested on MariaDB 10.1). If you want to setup FoolFuuka (to run alongside Ena), consider referring to the [FoolFuuka guide](https://wiki.bibanon.org/FoolFuuka) on the Bibliotheca Anonoma wiki.

Copy the default configuration file `ena.example.toml` to `ena.toml`. Add the boards you want to archive and adjust the other settings as necessary. Then, [install Rust](https://www.rust-lang.org/tools/install). Ena targets the latest stable version. Finally, compile and run Ena with:

```sh
cargo run --release
```

Note: The 4chan API guidelines state that you should "make API requests using the same protocol as the app." Since Ena uses HTTPS, any app using Ena in its backend should also use HTTPS.

## Logging

The default log level is `INFO`. Logging is configured by setting the `RUST_LOG` environment variable. For example, to turn on debug messages, use `RUST_LOG=ena=debug`. See the `env_logger` [documentation](https://docs.rs/env_logger/*/env_logger/) for more information.

## Differences from Asagi

[desuarchive's fork](https://github.com/desuarchive/asagi) is used as the reference for these comparisons.

### Scraping mechanics

* Existing posts in modified threads are only updated when the OP data, comment, or spoiler flag changes
* [xxHash](https://cyan4973.github.io/xxHash/) is used to check for comment differences instead of holding the comment in memory
* On start, all live threads are fetched and updated, regardless of whether they've changed or not
* On start, all archived threads are fetched and updated if they are not marked as archived in the database
* Closed threads remain locked even after they are archived (In Asagi, closed threads are unlocked on the refetch after archival)
* The `exif` column (a JSON blob of exif data, unique IPs, `since4pass`, and troll countries) is not used
* The old media/thumbs directory structure is not supported
* The "anchor thread" heuristic is used instead of the "page threshold" heuristic for determining when a thread was bumped off and when it was deleted
* When possible, the `timestamp_expired` for a deleted thread or post is taken from the `Last-Modified` header of the request, and not the time at which it was processed
* Bypassing the Cloudflare "I'm Under Attack Mode" JS challenge is not supported

### Post/media processing

* `[sjis]`, `[qstcolor]`, `[i]`, and `[u]` tags are supported. `/qst/` bold and italic text is also supported
* The `XX` and `A1` country flags are not ignored
* A fixed set of HTML character references ("entities") are replaced in usernames and titles (In addition to the references Ena replaces, Asagi also replaces all numeric character references of the form `&#\d+;`)
* Posts are not trimmed of whitespace (Asagi trims whitespace from the start and end of each line)
* Setting the group file permission (`webserverGroup`) of downloaded media is not supported
* Media requests that fail from recoverable errors (e.g. not a 404) are retried with exponential backoff
* API data must be complete and correct for it to be processed. Data with incorrect types, missing fields, or other errors is silently rejected during deserialization. For example, if the media of a post had no thumbnail, and the `tn_w` and `tn_h` fields were omitted, Ena would not replace them with defaults of 0. Instead, the media would be ignored, even if the full file existed

### Database

* If the OP post of a thread is moved to the `%%BOARD%%_deleted` table, no new posts from that thread will be inserted
* If a live thread is moved to the `%%BOARD%%_deleted` while Ena is running, Ena will continue to monitor it and produce errors while trying to update it. However, no data will actually be written
* `media_filename` is not updated when existing posts are updated
* PostgreSQL is not supported
* The `%%BOARD%%_daily` and `%%BOARD%%_users` tables are not created

## Known defects

Ena strives to be an accurate scraper, but it isn't perfect.

### Scraping mechanics

Though the bumped off/deleted detection should be better than Asagi in most cases, it still has flaws: (If absolute accuracy is required a `HEAD` request would have to be sent for each thread. For archived boards, `archive.json` could be polled instead.)
  * If `poll_interval` is too long or the scraped board moves too quickly, deleted threads may not be marked as bumped off
  * If the last _n_ threads from the catalog are deleted, modified, or bumped off, then those deleted threads will be marked as bumped off. In this way, it's technically possible for an entire board to be deleted and for every thread to be marked as bumped off (this could happen as a response to a raid)
  * If no anchor is found (every thread is new or modified), all removed threads are assumed to have been bumped off
  * Moved threads will be marked as deleted. This is because they disappear from a board just like a deleted thread would

### Data loss

* Threads and posts deleted on 4chan while Ena is stopped will not be marked as deleted when Ena restarts
* If Ena crashes in the process of updating an archived thread, on restart the thread may be marked as "archived" even if the update never happened. Thus, changes between the last poll of the thread and the archival of it may be lost
* Media are only downloaded the first time they or the post they are in is seen. This guards against duplicate media. But, if Ena crashes while media are queued to download, on restart they will not be requeued. Thus, those media never be downloaded
* Ena is not smart enough to notice large amounts of errors (e.g. if there's a network failure). So, it will just retry requests until all attempts are used up and the request queues empty out. Again, a long enough outage could lose media

## Legal

This program is licensed under the AGPLv3. See the `LICENSE` file for more information.

This program contains code from:

* [Asagi](https://github.com/desuarchive/asagi) (GPLv3)
* [futures-rs](https://github.com/rust-lang-nursery/futures-rs) (MIT)

See the `NOTICE` file for more information.

This program is completely unofficial and not affiliated with 4chan in any way.

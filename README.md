# Ena

A 4chan scraper. Currently designed to be a (mostly) compatible replacement for Asagi.

## Differences from Asagi

### Scraping Mechanics

* Existing posts in modified threads are only updated when the OP data, comment, or spoiler flag changes
* Closed threads remain locked even after they are archived (In Asagi, closed threads are unlocked on the refetch after archival)
* The `exif` column (a JSON blob of exif data, unique IPs, `since4pass`, and troll countries) is not used
* The old media/thumbs directory structure is not supported
* An improved bumped off/deleted algorithm
* When possible, the `timestamp_expired` for a deleted thread or post is taken from the `Last-Modified` header of the request, and not the time at which it was processed

### Post/media processing

* Unknown HTML tags are stripped from the output instead of being ignored
* Posts are trimmed of whitespace (may cause blankposts to become empty, non-NULL strings)
* Setting the group (`webserverGroup`) of downloaded media is currently not supported
* Duplicate images (those that share the same `media_hash`) are not downloaded

### Database

* PostgreSQL is not supported
* The `%%BOARD%%_daily` and `%%BOARD%%_users` tables are not created


## Logging

Logging is configured by setting the `RUST_LOG` environment variable. For example, to turn on all logging, use `RUST_LOG=ena`. Or, to just show warnings and errors, use `RUST_LOG=ena=warn`. See the `env_logger` [documentation](https://docs.rs/env_logger/*/env_logger/#enabling-logging) for more information.

## Legal

This program is licensed under the AGPLv3. See the `LICENSE` file for more information.

This program contains code from:

* [Asagi](https://github.com/desuarchive/asagi) (GPLv3)
* [futures-rs](https://github.com/rust-lang-nursery/futures-rs) (MIT)
* [html5ever](https://github.com/servo/html5ever) (MIT)

See the `NOTICE` file for more information.

This program is completely unofficial and not affiliated with 4chan in any way.

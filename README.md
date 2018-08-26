# Ena

A 4chan scraper. Currently designed to be a (mostly) compatible replacement for Asagi.

## Differences from Asagi

* The `exif` column is not stored (JSON blob of exif data, unique IPs, `since4pass`, and troll countries)
* Unknown HTML tags are stripped from the output instead of being ignored
* Posts are trimmed of whitespace (may cause blankposts to become empty, non-NULL strings)
* Existing posts in modified threads are only updated when the OP data changes or when the length of a comment changes (simple heuristic to detect when "(USER WAS BANNED FOR THIS POST)" is added to a post)
* Closed posts remain locked even after they are archived (In Asagi, closed posts are unlocked on the refetch after archival)
* The old media/thumbs directory structure is not supported
* A different bumped off/deleted algorithm
* When possible, the `timestamp_expired` for a deleted thread is taken from the `Last-Modified` header of the `threads.json` poll, and not the time at which the deleted thread was processed
* PostgreSQL is currently not supported

## Legal

This program is licensed under the AGPLv3. See the `LICENSE` file for more information.

This program contains code from [Asagi](https://github.com/desuarchive/asagi), which is licensed under the GPLv3, and [html5ever](https://github.com/servo/html5ever) which is licensed under the MIT License. See the `NOTICE` file for more information.

This program is completely unofficial and not affiliated with 4chan in any way.

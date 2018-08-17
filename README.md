# Ena

A 4chan scraper. Currently designed to be (mostly) compatible with Asagi/FoolFuuka.

## Asagi Incompatibilities

* `exif` column is not stored (JSON blob of exif data, unique IPs, `since4pass`, and troll countries)
* Unknown HTML tags are stripped from the output instead of being ignored
* Posts are trimmed of whitespace (may cause blankposts to become empty, non-NULL strings)

## Legal

This program is licensed under the AGPLv3. See the `LICENSE` file for more information.

This program contains code from [Asagi](https://github.com/desuarchive/asagi), which is licensed under the GPLv3, and [html5ever](https://github.com/servo/html5ever) which is licensed under the MIT License. See the `NOTICE` file for more information.

This program is completely unofficial and not affiliated with 4chan in any way.

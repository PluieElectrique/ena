//! 4chan HTML unescaping and cleaning (HTML to BBCode conversion).

// We use trivial regexes because of useful methods like is_match and replace_all, which are much
// faster than their std equivalents.
#![allow(clippy::trivial_regex)]

use std::borrow::Cow;

use lazy_static::lazy_static;
use log::Level;
use pest::{iterators::Pairs, Parser};
use pest_derive::Parser;
use regex::Regex;

use crate::four_chan::Board;

mod tests;

#[derive(Parser)]
#[grammar = "html/html.pest"]
struct HtmlParser;

lazy_static! {
    static ref ENTITY_CHECK: Regex = Regex::new("&").unwrap();
    static ref ENTITIES: Regex = Regex::new("&gt;|&#039;|&quot;|&lt;|&amp;|&[[:alnum:]#]+;").unwrap();
    static ref TAG_CHECK: Regex = Regex::new("<").unwrap();
    static ref REMOVED_TAGS: Regex = Regex::new(
        // Links include plain links and `<a class="quotelink">`
        r#"<wbr>|<a[^>]*>|</a>|<br><br><span class="abbr">.*?</span><br><table class="exif".*?</table>"#,
    ).unwrap();
    static ref SIMPLE_TAGS: Regex = Regex::new("<br>|<s>|</s>|<b>|</b>|<i>|</i>|<u>|</u>").unwrap();
    // It's tricky to match unknown elements, so we only match the tags and skip the contents
    static ref UNKNOWN_TAG: Regex = Regex::new("<[^>]+>").unwrap();
}

/// Unescape (some) HTML entities. If warnings are enabled, the board and post number from `context`
/// is printed to trace unknown entities back to their origins.
pub fn unescape(input: String, context: Option<(Board, u64)>) -> String {
    if !ENTITY_CHECK.is_match(&input) {
        return input;
    }

    // Asagi does a general `&#dddd;` escape, but the only numeric character reference we should
    // need to worry about is the apostrophe.
    let mut output = String::new();
    let mut pos = 0;
    for m in ENTITIES.find_iter(&input) {
        output.push_str(&input[pos..m.start()]);
        match m.as_str() {
            "&gt;" => output.push('>'),
            "&#039;" => output.push('\''),
            "&quot;" => output.push('"'),
            "&lt;" => output.push('<'),
            "&amp;" => output.push('&'),
            unknown => {
                if log_enabled!(Level::Warn) {
                    warn!(
                        "{}Unknown entity: {}",
                        context.map_or(String::new(), |context| format!(
                            "/{}/ No. {}: ",
                            context.0, context.1
                        )),
                        unknown
                    );
                }
                output.push_str(unknown);
            }
        }
        pos = m.end();
    }
    output.push_str(&input[pos..]);

    output
}

/// Clean comments by unescaping entities, converting tags to BBCode, and leaving other tags
/// unchanged. The board and post number from `context` is printed at the start of messages about
/// failed parses or unknown tags to trace errors back to their origins.
pub fn clean(input: String, context: Option<(Board, u64)>) -> String {
    if !TAG_CHECK.is_match(&input) {
        return unescape(input, context);
    }

    let removed = REMOVED_TAGS.replace_all(&input, "");

    let serialized = HtmlParser::parse(Rule::html, &removed)
        .map(|parse| {
            let mut serialized = String::new();
            serialize(&mut serialized, parse);
            Cow::Owned(serialized)
        })
        .unwrap_or_else(|err| {
            error!(
                "{}Failed to parse HTML: {:?}",
                context.map_or(String::new(), |context| format!(
                    "/{}/ No. {}: ",
                    context.0, context.1
                )),
                err
            );
            removed
        });

    let replaced = if SIMPLE_TAGS.is_match(&serialized) {
        let mut output = String::new();
        let mut pos = 0;
        for m in SIMPLE_TAGS.find_iter(&serialized) {
            output.push_str(&serialized[pos..m.start()]);
            match m.as_str() {
                "<br>" => output.push('\n'),
                "<s>" => output.push_str("[spoiler]"),
                "</s>" => output.push_str("[/spoiler]"),
                "<b>" => output.push_str("[b]"),
                "</b>" => output.push_str("[/b]"),
                "<i>" => output.push_str("[i]"),
                "</i>" => output.push_str("[/i]"),
                "<u>" => output.push_str("[u]"),
                "</u>" => output.push_str("[/u]"),
                _ => unreachable!(),
            }
            pos = m.end();
        }
        output.push_str(&serialized[pos..]);
        output
    } else {
        serialized.into_owned()
    };

    if log_enabled!(Level::Warn) && UNKNOWN_TAG.is_match(&replaced) {
        warn!(
            "{}Unknown tags: {:?}",
            context.map_or(String::new(), |context| format!(
                "/{}/ No. {}: ",
                context.0, context.1
            )),
            UNKNOWN_TAG
                .find_iter(&replaced)
                .map(|tag| tag.as_str())
                .collect::<Vec<_>>()
        );
    }

    unescape(replaced, context)
}

/// Serialize an AST generated by the Pest parser.
fn serialize(output: &mut String, pairs: Pairs<Rule>) {
    for pair in pairs {
        match pair.as_rule() {
            Rule::text => output.push_str(pair.as_str()),
            Rule::quote | Rule::deadlink => serialize(output, pair.into_inner()),
            Rule::fortune => {
                output.push_str("[fortune color=\"");
                let mut inner = pair.into_inner();
                output.push_str(inner.next().unwrap().as_str());
                output.push_str("\"]");
                output.push_str(inner.next().unwrap().as_str());
                output.push_str("[/fortune]");
            }
            Rule::shiftjis => {
                output.push_str("[shiftjis]");
                serialize(output, pair.into_inner());
                output.push_str("[/shiftjis]");
            }
            Rule::qst_italic => {
                output.push_str("[i]");
                serialize(output, pair.into_inner());
                output.push_str("[/i]");
            }
            Rule::qst_bold => {
                output.push_str("[b]");
                serialize(output, pair.into_inner());
                output.push_str("[/b]");
            }
            Rule::qst_color => {
                output.push_str("[qstcolor=");
                let mut inner = pair.into_inner();
                output.push_str(match inner.next().unwrap().as_rule() {
                    Rule::red => "red",
                    Rule::green => "green",
                    Rule::blue => "blue",
                    _ => unreachable!(),
                });
                output.push(']');
                serialize(output, inner);
                output.push_str("[/qstcolor]");
            }
            Rule::banned => {
                output.push_str("[banned]");
                serialize(output, pair.into_inner());
                output.push_str("[/banned]");
            }
            Rule::code => {
                output.push_str("[code]");
                serialize(output, pair.into_inner());
                output.push_str("[/code]");
            }
            Rule::other => {
                let mut inner = pair.into_inner();
                output.push_str(inner.next().unwrap().as_str());
                serialize(output, inner.next().unwrap().into_inner());
                output.push_str(inner.next().unwrap().as_str());
            }
            Rule::EOI => {}
            _ => unreachable!(),
        }
    }
}

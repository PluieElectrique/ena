#![cfg_attr(feature = "cargo-clippy", allow(trivial_regex))]

use html5ever::driver::ParseOpts;
use html5ever::rcdom::RcDom;
use html5ever::serialize::{AttrRef, Serialize, Serializer, TraversalScope};
use html5ever::tendril::TendrilSink;
use html5ever::{parse_fragment, LocalName, QualName};
use log::Level;
use regex::Regex;

use std::io::{self, Write};
use std::str;

mod tests;

use self::Color::*;
use self::FourChanTag::*;

lazy_static! {
    static ref FORTUNE_COLOR: Regex = Regex::new(r"color:#([[:xdigit:]]{3}{1,2})").unwrap();
    static ref BANNED_COLOR: Regex = Regex::new(r"color:\s*red").unwrap();
    static ref AMP: Regex = Regex::new(r"&").unwrap();
    static ref AMP_ENTITY: Regex = Regex::new(r"&amp;").unwrap();
    static ref APOS_ENTITY: Regex = Regex::new(r"&#039;").unwrap();
    static ref GT: Regex = Regex::new(r">").unwrap();
    static ref GT_ENTITY: Regex = Regex::new(r"&gt;").unwrap();
    static ref LT: Regex = Regex::new(r"<").unwrap();
    static ref LT_ENTITY: Regex = Regex::new(r"&lt;").unwrap();
    static ref QUOT: Regex = Regex::new("\"").unwrap();
    static ref QUOT_ENTITY: Regex = Regex::new(r"&quot;").unwrap();
    static ref NO_BREAK_SPACE: Regex = Regex::new("\u{a0}").unwrap();
    static ref NUMERIC_CHARACTER_REFERENCE: Regex =
        Regex::new(r"&#(?:x[[:xdigit:]]+|[[:digit:]]+);").unwrap();
}

/// Unescape HTML entities in subjects and names.
pub fn unescape(input: &str) -> String {
    // Asagi does a general `&#dddd;` escape, but the only character we should need to worry about
    // is the apostrophe.
    let input = APOS_ENTITY.replace_all(input, "'");
    let input = GT_ENTITY.replace_all(&input, ">");
    let input = LT_ENTITY.replace_all(&input, "<");
    let input = QUOT_ENTITY.replace_all(&input, "\"");

    if log_enabled!(Level::Warn) && NUMERIC_CHARACTER_REFERENCE.is_match(&input) {
        warn!("String contains unexpected entities: {}", input);
    }

    // Since each replace scans the entire string from the previous replace, it is important that we
    // replace the ampersand last. This way, we don't turn something like `&amp;gt;` into `>`
    let input = AMP_ENTITY.replace_all(&input, "&");

    input.to_string()
}

// It's a bit heavy-handed to use an HTML parser to clean a few types of tags. But, it is more
// versatile and reliable than regular expressions.
/// Clean comments by unescaping some entities, converting tags to BBCode, and serializing other tags.
pub fn clean(input: &str) -> io::Result<String> {
    let mut sink = vec![];

    let parser = parse_fragment(
        // TODO: Is RcDom too inefficient?
        RcDom::default(),
        ParseOpts::default(),
        QualName::new(None, ns!(html), local_name!("body")),
        vec![],
    );
    {
        let dom = parser.one(input);
        let html_elem = &dom.document.children.borrow()[0];
        let mut ser = HtmlSerializer::new(&mut sink);
        html_elem.serialize(&mut ser, TraversalScope::ChildrenOnly(None))?;
    }
    let mut string = String::from_utf8(sink).unwrap();
    // Remove trailing newlines from <br>'s before EXIF tables
    let len = string.trim_right().len();
    string.truncate(len);
    Ok(string)
}

#[derive(Debug, PartialEq)]
enum TagType {
    Start,
    End,
}

enum Color {
    Red,
    Green,
    Blue,
}

enum FourChanTag {
    /// (USER WAS BANNED/WARNED FOR THIS POST)
    Banned,
    /// `<b>` and `<span class="mu-s">` on /qst/
    Bold,
    /// The `<br>` tag
    Break,
    /// `<pre class="prettyprint">` on /g/
    Code,
    /// `<table class="exif">` and `<span class="abbr">` (holds the show/hide link) on /p/
    Exif,
    /// Colored fortunes on /s4s/
    Fortune(Option<String>),
    /// `<i>` and `<span class="mu-i">` on /qst/
    Italic,
    /// Plain links, `<a class="quotelink">`, and `<span class="deadlink">`
    Link,
    /// Colored text on /qst/
    QstColor(Color),
    /// A tag which prints its text and children, but not its tags or attributes. This is used for
    /// the root `<html>` element and is added to the stack when there is a missing parent.
    Quiet,
    /// `> implying`
    Quote,
    /// Shift_JIS art on /jp/ and /vip/
    ShiftJIS,
    ///	███████
    Spoiler,
    /// Supported by FoolFuuka, though not seen in the wild (yet)
    Subscript,
    /// Supported by FoolFuuka, though not seen in the wild (yet)
    Superscript,
    /// The `<u>` tag
    Underline,
    /// An unrecognized tag which is printed as-is. (Attributes may be reordered.)
    Unknown(LocalName),
    /// The `<wbr>` tag
    WordBreak,
}

impl FourChanTag {
    fn write<W: Write>(&self, w: &mut W, tag_type: &TagType) -> io::Result<()> {
        match self {
            // Tags that print nothing
            Exif | Link | Quiet | Quote | WordBreak => return Ok(()),
            Break => match *tag_type {
                TagType::Start => return w.write_all(b"\n"),
                TagType::End => return Ok(()),
            },
            Unknown(name) => {
                // start_elem handles printing the start tag so we don't have to copy the attributes
                assert_eq!(tag_type, &TagType::End);
                w.write_all(b"</")?;
                w.write_all(name.as_bytes())?;
                return w.write_all(b">");
            }
            _ => {}
        }
        w.write_all(b"[")?;
        if &TagType::End == tag_type {
            w.write_all(b"/")?;
        }
        let name = match self {
            Banned => "banned",
            Bold => "b",
            Code => "code",
            Fortune(_) => "fortune",
            Italic => "i",
            QstColor(_) => "qstcolor",
            ShiftJIS => "shiftjis",
            Spoiler => "spoiler",
            Subscript => "sub",
            Superscript => "sup",
            Underline => "u",
            _ => unreachable!(),
        };
        w.write_all(name.as_bytes())?;

        if &TagType::Start == tag_type {
            if let QstColor(color) = self {
                let color = match color {
                    Red => "red",
                    Green => "green",
                    Blue => "blue",
                };
                w.write_all(b"=")?;
                w.write_all(color.as_bytes())?;
            }
            if let Fortune(Some(color)) = self {
                w.write_all(b" color=\"#")?;
                w.write_all(color.as_bytes())?;
                w.write_all(b"\"")?;
            }
        }
        w.write_all(b"]")
    }
}

struct HtmlSerializer<W: Write> {
    writer: W,
    stack: Vec<FourChanTag>,
}

impl<W: Write> HtmlSerializer<W> {
    fn new(writer: W) -> Self {
        HtmlSerializer {
            writer,
            stack: vec![FourChanTag::Quiet],
        }
    }

    fn parent(&mut self) -> &FourChanTag {
        if self.stack.is_empty() {
            error!("HTML tag is missing parent. Putting a placeholder on the stack");
            self.stack.push(FourChanTag::Quiet);
        }
        self.stack.last_mut().unwrap()
    }
}

/// Escape a string according to the
/// [HTML spec](https://html.spec.whatwg.org/multipage/parsing.html#escapingString) for serializing
/// fragments.
fn escape_string(attr: &str, attr_mode: bool) -> String {
    let attr = AMP.replace_all(attr, "&amp;");
    let attr = NO_BREAK_SPACE.replace_all(&attr, "&nbsp;");
    if attr_mode {
        let attr = QUOT.replace_all(&attr, "&quot;");
        attr.to_string()
    } else {
        let attr = LT.replace_all(&attr, "&lt;");
        let attr = GT.replace_all(&attr, "&gt;");
        attr.to_string()
    }
}

impl<W: Write> Serializer for HtmlSerializer<W> {
    fn start_elem<'a, AttrIter>(&mut self, name: QualName, attrs: AttrIter) -> io::Result<()>
    where
        AttrIter: Iterator<Item = AttrRef<'a>>,
    {
        if let Exif = self.parent() {
            // Ignore all children
            self.stack.push(Exif);
            return Ok(());
        }

        let mut class = None;
        let mut style = None;
        let mut other_attrs = vec![];
        for (name, value) in attrs {
            match name.local {
                local_name!("class") => class = Some(value),
                local_name!("style") => style = Some(value),
                _ => other_attrs.push((name, value)),
            }
        }

        let tag = if let Some(class) = class {
            match (&name.local, class) {
                (local_name!("a"), "quotelink") => Link,
                (local_name!("span"), "deadlink") => Link,
                (local_name!("pre"), "prettyprint") => Code,
                (local_name!("table"), "exif") => Exif,
                (local_name!("span"), "abbr") => Exif,
                (local_name!("span"), "fortune") => {
                    let color = style
                        .and_then(|style| FORTUNE_COLOR.captures(style))
                        .and_then(|captures| captures.get(1))
                        .map(|m| m.as_str().to_string());
                    Fortune(color)
                }
                (local_name!("span"), "mu-s") => Bold,
                (local_name!("span"), "mu-i") => Italic,
                (local_name!("span"), "mu-r") => QstColor(Red),
                (local_name!("span"), "mu-g") => QstColor(Green),
                (local_name!("span"), "mu-b") => QstColor(Blue),
                (local_name!("span"), "quote") => Quote,
                (local_name!("span"), "sjis") => ShiftJIS,
                _ => Unknown(name.local.clone()),
            }
        } else if let Some(style) = style {
            match (&name.local, style) {
                // Using <b> for bans seems to be deprecated, but it's included for good measure
                (local_name!("b"), style) | (local_name!("strong"), style)
                    if BANNED_COLOR.is_match(style) =>
                {
                    Banned
                }
                _ => Unknown(name.local.clone()),
            }
        } else {
            match name.local {
                local_name!("a") => Link,
                local_name!("b") => Bold,
                local_name!("br") => Break,
                local_name!("i") => Italic,
                local_name!("s") => Spoiler,
                local_name!("sub") => Subscript,
                local_name!("sup") => Superscript,
                local_name!("u") => Underline,
                local_name!("wbr") => WordBreak,
                _ => Unknown(name.local.clone()),
            }
        };

        // https://html.spec.whatwg.org/multipage/parsing.html#serialising-html-fragments
        if let Unknown(name) = &tag {
            error!(
                "Unrecognized tag: {}, class: {:?}, style: {:?}, other: {:?}",
                name, class, style, other_attrs
            );

            self.writer.write_all(b"<")?;
            self.writer.write_all(name.as_bytes())?;
            if let Some(class) = class {
                self.writer.write_all(b" class=\"")?;
                self.writer
                    .write_all(escape_string(class, true).as_bytes())?;
                self.writer.write_all(b"\"")?;
            }
            if let Some(style) = style {
                self.writer.write_all(b" style=\"")?;
                self.writer
                    .write_all(escape_string(style, true).as_bytes())?;
                self.writer.write_all(b"\"")?;
            }
            for (name, value) in other_attrs {
                self.writer.write_all(b" ")?;
                self.writer.write_all(name.local.as_bytes())?;
                self.writer.write_all(b"=\"")?;
                self.writer
                    .write_all(escape_string(value, true).as_bytes())?;
                self.writer.write_all(b"\"")?;
            }
            self.writer.write_all(b">")?;
        } else {
            tag.write(&mut self.writer, &TagType::Start)?;
        }
        self.stack.push(tag);
        Ok(())
    }

    fn end_elem(&mut self, _name: QualName) -> io::Result<()> {
        let tag = match self.stack.pop() {
            Some(tag) => tag,
            None => {
                error!("HTML tag is missing parent. Skipping end tag");
                return Ok(());
            }
        };

        if let Unknown(name) = &tag {
            match name {
                local_name!("area")
                | local_name!("base")
                | local_name!("basefont")
                | local_name!("bgsound")
                | local_name!("col")
                | local_name!("embed")
                | local_name!("frame")
                | local_name!("hr")
                | local_name!("img")
                | local_name!("input")
                | local_name!("keygen")
                | local_name!("link")
                | local_name!("meta")
                | local_name!("param")
                | local_name!("source")
                | local_name!("track") => return Ok(()),
                _ => {}
            }
        }
        tag.write(&mut self.writer, &TagType::End)
    }

    // We don't support scripting so the text of <noscript> will always be escaped
    fn write_text(&mut self, text: &str) -> io::Result<()> {
        if let Exif = self.parent() {
            return Ok(());
        }

        let escape = if let Unknown(name) = self.parent() {
            match name {
                local_name!("style")
                | local_name!("script")
                | local_name!("xmp")
                | local_name!("iframe")
                | local_name!("noembed")
                | local_name!("noframes")
                | local_name!("plaintext") => false,
                _ => true,
            }
        } else {
            true
        };

        if escape {
            self.writer.write_all(escape_string(text, false).as_bytes())
        } else {
            self.writer.write_all(text.as_bytes())
        }
    }

    fn write_comment(&mut self, _text: &str) -> io::Result<()> {
        error!("HTML serializer tried to write comment");
        Ok(())
    }

    fn write_doctype(&mut self, _name: &str) -> io::Result<()> {
        error!("HTML serializer tried to write doctype");
        Ok(())
    }

    fn write_processing_instruction(&mut self, _target: &str, _data: &str) -> io::Result<()> {
        error!("HTML serializer tried to write processing instruction");
        Ok(())
    }
}

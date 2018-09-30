#![cfg(test)]

use super::{clean, unescape};

macro_rules! test_c {
    ($name:ident, $input:expr, $output:expr) => {
        #[test]
        fn $name() {
            assert_eq!($output, clean($input).unwrap());
        }
    };
}

macro_rules! test_u {
    ($name:ident, $input:expr, $output:expr) => {
        #[test]
        fn $name() {
            assert_eq!($output, unescape($input));
        }
    };
}

test_c!(
    banned_b,
    r#"<b style="color: red;">(USER WAS BANNED FOR THIS POST)</b>"#,
    "[banned](USER WAS BANNED FOR THIS POST)[/banned]"
);
test_c!(
    banned_strong,
    r#"<strong style="color: red;">(USER WAS BANNED FOR THIS POST)</strong>"#,
    "[banned](USER WAS BANNED FOR THIS POST)[/banned]"
);
test_c!(
    bold,
    r#"<b>bold</b> <span class="mu-s">bold</span>"#,
    "[b]bold[/b] [b]bold[/b]"
);
test_c!(br, "I<br>am<br>broken", "I\nam\nbroken");
test_c!(
    code,
    r#"<pre class="prettyprint">println!("&lt;p&gt;Goodbye, world.&lt;/p&gt;");</pre>"#,
    r#"[code]println!("&lt;p&gt;Goodbye, world.&lt;/p&gt;");[/code]"#
);
test_c!(
    deadlink,
    r#"<span class="deadlink">&gt;&gt;123456</span>"#,
    "&gt;&gt;123456"
);
test_c!(
    exif,
    r#"pic not related<br><br><span class="abbr">[EXIF data available. Click <a href="javascript:void(0)" onclick="toggle('exif12345')">here</a> to show/hide.]</span><br><table class="exif" id="exif12345"><tr><td colspan="2"><b>Camera-Specific Properties:</b></td></tr><tr><td colspan="2"><b></b></td></tr><tr><td>Camera Model</td><td>Model</td></tr><tr><td>Equipment Make</td><td>Make</td></tr><tr><td colspan="2"><b></b></td></tr><tr><td colspan="2"><b>Image-Specific Properties:</b></td></tr><tr><td colspan="2"><b></b></td></tr><tr><td>Image Created</td><td>2015:07:14 11:50:00</td></tr><tr><td>Image Orientation</td><td>Top, Left-Hand</td></tr><tr><td>Flash</td><td>No Flash</td></tr><tr><td>F-Number</td><td>f/8</td></tr><tr><td>Focal Length</td><td>10.00 mm</td></tr><tr><td>Exposure Bias</td><td>0 EV</td></tr><tr><td>White Balance</td><td>Manual</td></tr><tr><td>Image Width</td><td>1000</td></tr><tr><td>ISO Speed Rating</td><td>800</td></tr><tr><td>Image Height</td><td>1000</td></tr><tr><td>Exposure Time</td><td>1 sec</td></tr><tr><td colspan="2"><b></b></td></tr></table>"#,
    "pic not related"
);
test_c!(
    fortune,
    r#"<span class="fortune" style="color:#eef2ff"><br><br><b>Your fortune: You&#039;re gonna make it.</b></span>"#,
    "[fortune color=\"#eef2ff\"]\n\n[b]Your fortune: You're gonna make it.[/b][/fortune]"
);
test_c!(
    italic,
    r#"<i>italic</i> <span class="mu-i">italic</span>"#,
    "[i]italic[/i] [i]italic[/i]"
);
test_c!(link, r#"<a href="4chan.org">4chan.org</a>"#, "4chan.org");
test_c!(
    qstcolor,
    r#"<span class="mu-r">red</span> <span class="mu-g">green</span> <span class="mu-b">blue</span>"#,
    "[qstcolor=red]red[/qstcolor] [qstcolor=green]green[/qstcolor] [qstcolor=blue]blue[/qstcolor]"
);
test_c!(
    quote,
    r#"<span class="quote">&gt;implying</span>"#,
    "&gt;implying"
);
test_c!(
    quotelink,
    r##"<a href="#p123456" class="quotelink">&gt;&gt;123456</a>"##,
    "&gt;&gt;123456"
);
test_c!(
    shiftjis,
    r#"<span class="sjis">(╯°□°）╯︵ ┻━┻</span>"#,
    "[shiftjis](╯°□°）╯︵ ┻━┻[/shiftjis]"
);
test_c!(
    spoiler,
    "it is <s>great</s>",
    "it is [spoiler]great[/spoiler]"
);
test_c!(subscript, "<sub>submarine</sub>", "[sub]submarine[/sub]");
test_c!(
    superscript,
    "<sup>super-duper</sup>",
    "[sup]super-duper[/sup]"
);
test_c!(underline, "<u>underline</u>", "[u]underline[/u]");
test_c!(
    unknown,
    "<p>text</p><span style=\"font-size:1em;\">asdf</span><img alt='\"you\" &\u{a0}I' src=\"facepalm.jpg\">",
    r#"<p>text</p><span style="font-size:1em;">asdf</span><img alt="&quot;you&quot; &amp;&nbsp;I" src="facepalm.jpg">"#
);
test_c!(
    wbr,
    "an<wbr>ti<wbr>dis<wbr>es<wbr>tab<wbr>lish<wbr>ment<wbr>ar<wbr>i<wbr>an<wbr>ism",
    "antidisestablishmentarianism"
);

test_u!(escapes, "&lt;&#039;&amp;&quot;&gt;", r#"<'&">"#);
test_u!(
    complex_ampersand,
    "&amp;#039; &amp;gt; &amp;lt; &amp;quot; &amp;amp;",
    "&#039; &gt; &lt; &quot; &amp;"
);

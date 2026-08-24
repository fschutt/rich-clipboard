//! Integration tests for `rclip-html`.
//!
//! Malformed nesting, character references and the depth bound get the most
//! attention, because those are the three places an HTML reader silently
//! produces wrong output instead of failing — and because in clipboard markup
//! the first of them is not an edge case, it is Tuesday.

use std::fs;

use rclip_html::{
    css, element, entity, Color, Document, ErrorKind, HtmlText, RunText, Runs, Token, Tokenizer,
    Whitespace,
};

const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/synthetic/rclip-html/"
);

fn fixture(name: &str) -> Vec<u8> {
    fs::read(format!("{CORPUS}{name}")).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

/// Plain text through the owned API.
fn text(src: &[u8]) -> String {
    Document::parse(src)
        .unwrap_or_else(|e| panic!("should parse: {e}"))
        .text
}

/// Plain text through the borrowing API, which must agree.
fn borrowed_text(src: &[u8]) -> String {
    let mut out = String::new();
    for run in Runs::new(src) {
        match run.expect("should parse").text {
            RunText::Text(t) => out.extend(t.chars()),
            RunText::Break => out.push('\n'),
            RunText::Tab => out.push('\t'),
        }
    }
    out
}

fn err_of(src: &[u8]) -> ErrorKind {
    Document::parse(src)
        .err()
        .unwrap_or_else(|| panic!("expected an error, got a clean parse"))
        .kind
}

fn tokens(src: &[u8]) -> Vec<Token<'_>> {
    Tokenizer::new(src).collect()
}

/// Every distinct style in a document, in order, as `(text, style)`.
fn styled(src: &[u8]) -> Vec<(String, rclip_html::OwnedStyle)> {
    let doc = Document::parse(src).expect("should parse");
    doc.runs
        .iter()
        .map(|r| (doc.run_text(r).to_owned(), r.style.clone()))
        .collect()
}

// ---------------------------------------------------------------- tokenizer

#[test]
fn tags_attributes_text_and_comments() {
    let src = br#"<p class="a" id=b data-x='c'>hi<!-- note --></p>"#;
    let t = tokens(src);
    assert_eq!(t.len(), 4, "{t:#?}");
    let Token::StartTag(tag) = t[0] else {
        panic!("{:?}", t[0])
    };
    assert!(tag.is("P"), "tag names match case-insensitively");
    let attrs: Vec<_> = tag
        .attributes()
        .map(|a| (a.name, a.value.as_str().unwrap_or_default()))
        .collect();
    assert_eq!(attrs, [("class", "a"), ("id", "b"), ("data-x", "c")]);
    let Token::EndTag { name, offset } = t[3] else {
        panic!("{:?}", t[3])
    };
    assert_eq!(name, "p");
    assert_eq!(src.get(offset), Some(&b'<'), "the offset points at the `<`");
    assert!(matches!(t[2], Token::Comment(b" note ")));
}

#[test]
fn all_three_quoting_forms_and_a_boolean_attribute() {
    // The unquoted form is what hand-written and mail-client markup uses.
    // Refusing it loses the styling it carried, which is the whole point of
    // being here.
    let src = br#"<input a="1" b='2' c=3 disabled>"#;
    let Token::StartTag(tag) = tokens(src)[0] else {
        panic!()
    };
    let attrs: Vec<_> = tag
        .attributes()
        .map(|a| (a.name, a.value.as_str().unwrap_or_default()))
        .collect();
    assert_eq!(
        attrs,
        [("a", "1"), ("b", "2"), ("c", "3"), ("disabled", "")]
    );
}

#[test]
fn a_gt_inside_a_quoted_attribute_does_not_end_the_tag() {
    let src = br#"<a title="x>y">z</a>"#;
    let Token::StartTag(tag) = tokens(src)[0] else {
        panic!()
    };
    assert_eq!(tag.attr("title").and_then(|v| v.as_str()), Some("x>y"));
    assert_eq!(text(src), "z");
}

#[test]
fn a_lone_less_than_is_text() {
    // `a < b` is prose, not a tag, and every browser shows it as prose.
    assert_eq!(text(b"a < b and 3<4"), "a < b and 3<4");
}

#[test]
fn raw_text_elements_are_not_markup_inside() {
    // Every browser puts a `<style>` block at the top of a clipboard fragment.
    // A reader that lexed its contents pastes a stylesheet into the document,
    // and one that lexed `a<b` inside a `<script>` loses the rest of the file.
    assert_eq!(
        text(b"<style>p{color:red}\na<b{}</style><script>if (a<b) {}</script>after"),
        "after"
    );
    assert_eq!(text(b"<title>Tab title</title>body"), "body");
}

#[test]
fn a_truncated_tag_is_not_an_error() {
    // A clipboard transfer that was cut short is a paste that should still put
    // something down.
    assert_eq!(text(b"<p>text<span style=\"color:red"), "text");
    assert_eq!(text(b"<p>text<"), "text<");
    assert_eq!(text(b"<!-- unterminated"), "");
}

#[test]
fn nothing_in_the_crate_recurses_on_input() {
    // Three shapes that a hand-written lexer is tempted to handle by calling
    // itself: a run of characters that are not an attribute name, an end tag
    // with no name, and an empty raw-text element. One `self.next()` per
    // repetition would put the stack depth under the control of the input,
    // which is the same class of bug as unbounded nesting and has the same
    // consequence.
    for src in [
        [b"<a ".to_vec(), b"=".repeat(200_000), b">x".to_vec()].concat(),
        [b"</>".repeat(200_000), b"x".to_vec()].concat(),
        [b"<style></style>".repeat(200_000), b"x".to_vec()].concat(),
        b"<".repeat(200_000),
    ] {
        let doc = Document::parse(&src).expect("no depth limit is involved");
        assert!(
            doc.text.ends_with('x') || doc.text.starts_with('<'),
            "{:?}",
            &doc.text[..4.min(doc.text.len())]
        );
    }
}

#[test]
fn the_tokenizer_always_consumes_something() {
    // The one property that makes a hand-written lexer safe to run on hostile
    // input: no byte sequence can make it stand still.
    for src in [
        &b"<"[..],
        b"</",
        b"</>",
        b"<>",
        b"< >",
        b"<!",
        b"<!-",
        b"<?",
        b"<a",
        b"<a ",
        b"<a =",
        b"<a b=",
        b"<a b='",
        b"&",
        b"&#",
        b"&#x",
        b"\xff\xfe",
    ] {
        let mut tok = Tokenizer::new(src);
        let mut steps = 0;
        while tok.next().is_some() {
            steps += 1;
            assert!(steps <= src.len() + 4, "did not terminate on {src:?}");
        }
    }
}

// ------------------------------------------------------------- entities

#[test]
fn named_numeric_and_hexadecimal_references() {
    assert_eq!(
        text(b"&amp; &lt; &gt; &quot; &apos; &copy; &mdash; &hellip;"),
        "& < > \" ' \u{a9} \u{2014} \u{2026}"
    );
    assert_eq!(text(b"&#65;&#x42;&#X43;"), "ABC");
    assert_eq!(text(b"&nbsp;").as_bytes(), "\u{a0}".as_bytes());
}

#[test]
fn the_c1_range_is_windows_1252_and_not_a_control_character() {
    // `&#150;` means an en dash, because the producer was a Windows
    // application that confused a code page with Unicode. HTML5 writes the
    // mis-mapping into the spec because every browser had to implement it; a
    // parser that decodes it to U+0096 puts an invisible control character
    // where the document meant punctuation.
    assert_eq!(
        text(b"&#150;&#151;&#147;&#148;&#128;"),
        "\u{2013}\u{2014}\u{201c}\u{201d}\u{20ac}"
    );
}

#[test]
fn out_of_range_references_become_the_replacement_character() {
    assert_eq!(text(b"&#0;&#x110000;&#xD800;"), "\u{fffd}\u{fffd}\u{fffd}");
    // Forty digits is not a code point; it must clamp rather than wrap into a
    // valid scalar.
    assert_eq!(
        text(b"&#11111111111111111111111111111111111111111;"),
        "\u{fffd}"
    );
}

#[test]
fn a_missing_semicolon_still_resolves_and_takes_the_longest_name() {
    // `&notin` is `&notin;`, not `&not;` followed by `in`. Getting this
    // backwards turns a set-theory symbol into `¬in`.
    assert_eq!(text(b"&notin"), "\u{2209}");
    assert_eq!(text(b"&not"), "\u{ac}");
    assert_eq!(text(b"&amp&lt"), "&<");
}

#[test]
fn what_is_not_a_reference_stays_literal() {
    assert_eq!(text(b"&nosuchentity; a & b &"), "&nosuchentity; a & b &");
    assert_eq!(text(b"&#;"), "&#;");
}

#[test]
fn the_named_table_is_sorted() {
    // Load-bearing: the lookup binary-searches it, so an unsorted table is a
    // silently wrong lookup rather than a compile error.
    for pair in entity::NAMED.windows(2) {
        assert!(pair[0].0 < pair[1].0, "out of order: {:?}", pair);
    }
}

// ------------------------------------------------------------- whitespace

#[test]
fn whitespace_collapses_the_way_a_browser_shows_it() {
    // Without this, a fragment copied out of a browser pastes with a newline
    // and four spaces between every two words, because the serializer
    // pretty-prints it.
    assert_eq!(text(b"<p>Hello   \n   world</p>"), "Hello world");
    assert_eq!(text(b"  \n  <p>  leading  </p>"), "leading");
    assert_eq!(text(b"<b>a </b><i> b</i>"), "a b", "one space, not two");
}

#[test]
fn pre_keeps_its_whitespace() {
    assert_eq!(
        text(b"<p>a   b</p><pre>  two\n\ttab</pre><p>c</p>"),
        "a b\n  two\n\ttab\nc"
    );
}

#[test]
fn crlf_normalizes_to_lf_inside_pre() {
    assert_eq!(text(b"<pre>a\r\nb\rc</pre>"), "a\nb\nc");
}

// ---------------------------------------------------------------- breaks

#[test]
fn one_br_is_one_newline() {
    // `<br>` is a line break, not a block boundary. Counting it as both is the
    // obvious bug and it doubles every line in the document.
    assert_eq!(text(b"a<br>b<br><br>c"), "a\nb\n\nc");
}

#[test]
fn block_boundaries_do_not_double_up_or_dangle() {
    assert_eq!(text(b"<div><p>one</p><p>two</p></div>"), "one\ntwo");
    assert_eq!(text(b"<p>only</p>"), "only", "no leading or trailing break");
    assert_eq!(text(b"<ul><li>a<li>b</ul>"), "a\nb", "<li> closes <li>");
    assert_eq!(text(b"<p>a<p>b<p>c"), "a\nb\nc", "<p> closes <p>");
}

#[test]
fn table_cells_are_separated_by_tabs_and_rows_by_newlines() {
    // A table pasted one cell per line reads worse than the original; a table
    // pasted with the cells run together is unreadable.
    assert_eq!(
        text(b"<table><tr><td>a1</td><td>b1</td></tr><tr><td>a2</td><td>b2</td></tr></table>"),
        "a1\tb1\na2\tb2"
    );
    assert_eq!(text(b"<td>alone</td>"), "alone", "no leading tab");
}

// ---------------------------------------------------------- malformed nesting

#[test]
fn mismatched_nesting_is_the_normal_case_and_not_an_error() {
    // `<b><i></b></i>` is what contenteditable, mail clients and Word's HTML
    // export produce all day.
    let runs = styled(b"<b><i>both</b>italic</i>plain");
    assert_eq!(text(b"<b><i>both</b>italic</i>plain"), "bothitalicplain");
    assert_eq!(runs[0].0, "both");
    assert!(runs[0].1.bold && runs[0].1.italic);
    assert_eq!(runs[1].0, "italic");
    assert!(
        !runs[1].1.bold && runs[1].1.italic,
        "</b> closed the <i> above it, and the later </i> matched nothing"
    );
    assert!(runs[2].1.is_default());
}

#[test]
fn an_end_tag_with_nothing_open_closes_nothing() {
    // Closing the innermost element instead would turn one stray end tag into
    // a document where every later style is off by one.
    let runs = styled(b"<b>a</i>b</b>c");
    assert_eq!(runs[0].0, "ab");
    assert!(runs[0].1.bold);
    assert!(runs[1].1.is_default());
}

#[test]
fn an_unclosed_element_still_styles_the_rest() {
    let runs = styled(b"plain <b>bold to the end");
    assert!(runs[1].1.bold);
}

#[test]
fn a_void_element_never_goes_on_the_stack() {
    // `<img>` left open would swallow the rest of the document into itself,
    // which for a fragment full of images is most of the fragment.
    let runs = styled(b"<b>a</b><img src=x><i>b</i>");
    assert!(runs[0].1.bold && !runs[0].1.italic);
    assert!(runs[1].1.italic && !runs[1].1.bold);
}

#[test]
fn nesting_past_the_limit_is_an_error_and_not_a_stack_overflow() {
    let bomb: Vec<u8> = b"<div>".repeat(200).into_iter().chain(*b"x").collect();
    assert_eq!(err_of(&bomb), ErrorKind::DepthLimit);
    // And through the borrowing API, which is where the stack actually lives.
    assert!(Runs::new(&bomb).any(|r| r.is_err()));
}

#[test]
fn a_document_at_exactly_the_limit_still_parses() {
    let depth = usize::try_from(rclip_core::MAX_DEPTH).unwrap();
    let ok: Vec<u8> = b"<div>".repeat(depth).into_iter().chain(*b"x").collect();
    assert_eq!(text(&ok), "x");
}

// ------------------------------------------------------------------ styling

#[test]
fn the_inline_formatting_elements() {
    for (src, check) in [
        (&b"<b>x</b>"[..], "bold"),
        (b"<strong>x</strong>", "bold"),
        (b"<i>x</i>", "italic"),
        (b"<em>x</em>", "italic"),
        (b"<u>x</u>", "underline"),
        (b"<ins>x</ins>", "underline"),
        (b"<s>x</s>", "strike"),
        (b"<strike>x</strike>", "strike"),
        (b"<del>x</del>", "strike"),
    ] {
        let style = styled(src).remove(0).1;
        let got = match check {
            "bold" => style.bold,
            "italic" => style.italic,
            "underline" => style.underline,
            _ => style.strike,
        };
        assert!(got, "{}: {style:?}", String::from_utf8_lossy(src));
    }
}

#[test]
fn style_attributes_beat_the_element_and_the_presentational_attributes() {
    // `<font color="red" style="color:blue">` is blue everywhere it is
    // rendered, because `style=` is the more specific of the two.
    let style = styled(br#"<font color="red" style="color:#0000ff">x</font>"#)
        .remove(0)
        .1;
    assert_eq!(style.color, Some(Color::new(0, 0, 255)));

    // And a `style=` can turn an element's own formatting back off.
    let style = styled(br#"<b style="font-weight:normal">x</b>"#)
        .remove(0)
        .1;
    assert!(!style.bold);
}

#[test]
fn every_style_property_this_crate_reads() {
    let style = styled(
        br#"<span style="font-weight:700;font-style:italic;text-decoration:underline line-through;color:rgb(1,2,3);background-color:#ff0;font-size:14pt;font-family:&quot;Courier New&quot;, monospace">x</span>"#,
    )
    .remove(0)
    .1;
    assert!(style.bold && style.italic && style.underline && style.strike);
    assert_eq!(style.color, Some(Color::new(1, 2, 3)));
    assert_eq!(style.background, Some(Color::new(255, 255, 0)));
    assert_eq!(style.size_pt, Some(14.0));
    assert_eq!(
        style.font_family.as_deref(),
        Some("Courier New"),
        "the &quot; entities and the fallback chain both have to come off"
    );
}

#[test]
fn an_entity_semicolon_is_not_a_declaration_separator() {
    // The bug this guards is self-inflicted: this workspace's own HTML writer
    // emits `font-family:&quot;Name&quot;`, and a splitter that cut on the
    // entity's `;` would turn one declaration into three and the font name
    // into a quote mark.
    let style = styled(br#"<span style="font-family:&quot;A B&quot;;color:red">x</span>"#)
        .remove(0)
        .1;
    assert_eq!(style.font_family.as_deref(), Some("A B"));
    assert_eq!(style.color, Some(Color::new(255, 0, 0)));
}

#[test]
fn text_decoration_replaces_rather_than_adds() {
    // One `text-decoration` declaration is the whole value, so `underline` on
    // a child of a struck-through parent turns the strike off. A reader that
    // ORed them together would keep a line the author removed.
    let runs = styled(br#"<s><span style="text-decoration:underline">x</span></s>"#);
    assert!(runs[0].1.underline && !runs[0].1.strike);
}

#[test]
fn colour_syntaxes() {
    use css::ColorValue::{Rgb, Transparent};
    assert_eq!(css::color(b"#f00"), Some(Rgb(Color::new(255, 0, 0))));
    assert_eq!(css::color(b"#Ff0000"), Some(Rgb(Color::new(255, 0, 0))));
    assert_eq!(css::color(b"rgb(1, 2, 3)"), Some(Rgb(Color::new(1, 2, 3))));
    assert_eq!(
        css::color(b"rgb(1 2 3 / 50%)"),
        Some(Rgb(Color::new(1, 2, 3)))
    );
    assert_eq!(
        css::color(b"rgba(4,5,6,0.5)"),
        Some(Rgb(Color::new(4, 5, 6)))
    );
    assert_eq!(css::color(b"RED"), Some(Rgb(Color::new(255, 0, 0))));
    // Three ways of saying "no colour at all", which is not the same as black:
    // folding them to black is how a highlight appears where none was asked
    // for.
    assert_eq!(css::color(b"transparent"), Some(Transparent));
    assert_eq!(css::color(b"rgba(0,0,0,0)"), Some(Transparent));
    assert_eq!(css::color(b"#00000000"), Some(Transparent));
    assert_eq!(css::color(b"papayawhip"), None, "unlisted names inherit");
    assert_eq!(css::color(b"url(x.png)"), None);
}

#[test]
fn a_transparent_background_clears_an_inherited_one() {
    let runs = styled(
        br#"<span style="background:#ff0"><span style="background:transparent">x</span></span>"#,
    );
    assert_eq!(runs[0].1.background, None);
}

#[test]
fn font_size_units() {
    assert_eq!(css::font_size_pt(b"12pt", None), Some(12.0));
    assert_eq!(
        css::font_size_pt(b"16px", None),
        Some(12.0),
        "96dpi CSS pixels"
    );
    assert_eq!(css::font_size_pt(b"1in", None), Some(72.0));
    assert_eq!(css::font_size_pt(b"medium", None), Some(12.0));
    assert_eq!(css::font_size_pt(b"2em", Some(10.0)), Some(20.0));
    assert_eq!(css::font_size_pt(b"150%", Some(10.0)), Some(15.0));
    assert_eq!(
        css::font_size_pt(b"12", None),
        None,
        "a bare number is not a length"
    );
    assert_eq!(css::font_size_pt(b"0pt", None), None);
    assert_eq!(css::font_size_pt(b"-4pt", None), None);
    assert_eq!(css::font_size_pt(b"inherit", None), None);
}

#[test]
fn relative_sizes_compose_against_the_enclosing_element() {
    let runs =
        styled(br#"<span style="font-size:20pt"><span style="font-size:50%">x</span></span>"#);
    assert_eq!(runs[0].1.size_pt, Some(10.0));
}

#[test]
fn the_legacy_font_element() {
    let style = styled(br#"<font face="Georgia" color="red" size="5">x</font>"#)
        .remove(0)
        .1;
    assert_eq!(style.font_family.as_deref(), Some("Georgia"));
    assert_eq!(style.color, Some(Color::new(255, 0, 0)));
    assert_eq!(
        style.size_pt,
        Some(18.0),
        "size 5 is 24px on the 1..7 scale"
    );

    // `+N` / `-N` are relative to size 3.
    assert_eq!(css::font_attr_size_pt(b"-1"), css::font_attr_size_pt(b"2"));
    assert_eq!(css::font_attr_size_pt(b"+2"), css::font_attr_size_pt(b"5"));
    assert_eq!(css::font_attr_size_pt(b"99"), css::font_attr_size_pt(b"7"));
}

#[test]
fn bgcolor_still_works() {
    let style = styled(br##"<td bgcolor="#eee">x</td>"##).remove(0).1;
    assert_eq!(style.background, Some(Color::new(238, 238, 238)));
}

#[test]
fn headings_and_table_headers_are_bold() {
    // Not an invention: it is in the UA stylesheet of every browser that ever
    // put a fragment on a clipboard. The *size* increase is deliberately not
    // applied, because that one really would be inventing a number.
    for src in [&b"<h1>x</h1>"[..], b"<h6>x</h6>", b"<th>x</th>"] {
        assert!(styled(src).remove(0).1.bold, "{:?}", src);
    }
    assert!(!styled(b"<h7>x</h7>").remove(0).1.bold, "there is no h7");
}

#[test]
fn declarations_split_on_the_right_semicolons() {
    let block = br#"a:1;b:'x;y';c:url(p;q);;d:2;e"#;
    let got: Vec<_> = css::declarations(block)
        .map(|d| (d.name, String::from_utf8_lossy(d.value).into_owned()))
        .collect();
    assert_eq!(
        got,
        [
            ("a", "1".to_owned()),
            ("b", "'x;y'".to_owned()),
            ("c", "url(p;q)".to_owned()),
            ("d", "2".to_owned()),
        ],
        "quotes and parens hold, an empty declaration and one with no colon are dropped"
    );
}

// -------------------------------------------------------------------- text

#[test]
fn as_str_is_a_fast_path_and_never_a_wrong_answer() {
    let plain = HtmlText::new(b"hello", Whitespace::Collapse, false);
    assert_eq!(plain.as_str(), Some("hello"));
    // Anything that needs work says so rather than handing back the raw bytes.
    for raw in [&b"a&amp;b"[..], b"a  b", b"a\nb"] {
        let t = HtmlText::new(raw, Whitespace::Collapse, false);
        assert_eq!(t.as_str(), None, "{raw:?}");
        assert!(
            t.chars().collect::<String>().len() <= raw.len(),
            "decoding never grows a span"
        );
    }
}

#[test]
fn invalid_utf8_is_replaced_and_not_refused() {
    // A clipboard payload comes from another process. One replacement
    // character is a better outcome than Ctrl+V doing nothing.
    let out = text(b"<p>caf\xff \xc3\xa9 \xe2\x82</p>");
    assert!(out.starts_with("caf\u{fffd}"), "{out:?}");
    assert!(out.contains('\u{e9}'), "{out:?}");
}

#[test]
fn the_borrowing_and_owned_apis_agree() {
    for name in fixture_names() {
        let bytes = fixture(&name);
        let Ok(doc) = Document::parse(&bytes) else {
            continue;
        };
        // `Document` trims one trailing space that the run stream still has.
        assert_eq!(
            borrowed_text(&bytes).trim_end_matches(' '),
            doc.text,
            "{name}"
        );
    }
}

// ---------------------------------------------------------------- fixtures

fn fixture_names() -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(CORPUS)
        .expect("corpus directory")
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension()? == "bin").then(|| p.file_name()?.to_str().map(str::to_owned))?
        })
        .collect();
    out.sort();
    assert!(
        out.len() >= 10,
        "expected the whole fixture set, got {out:?}"
    );
    out
}

#[test]
fn the_fixtures_decode_to_what_their_sidecars_say() {
    assert_eq!(
        text(&fixture("styled-inline.bin")),
        "plain bold italic under struck span"
    );
    assert_eq!(
        text(&fixture("browser-fragment.bin")),
        "The quick brown fox jumps over the lazy dog.\nCaf\u{e9} \u{2014} na\u{ef}ve \u{2014} 50%"
    );
    assert_eq!(
        text(&fixture("malformed-nesting.bin")),
        "bothitalicplaintail"
    );
    assert_eq!(text(&fixture("font-element.bin")), "big serif\tsmall");
    assert_eq!(
        text(&fixture("blocks-and-breaks.bin")),
        "one\ntwo\na\nb\n\nc\nfirst\nsecond\nHeading\nafter"
    );
    assert_eq!(
        text(&fixture("pre-whitespace.bin")),
        "collapsed text\n  two spaces\n\tand a tab\nafter"
    );
    assert_eq!(text(&fixture("unquoted-attributes.bin")), "xyz");
    assert_eq!(text(&fixture("truncated-tag.bin")), "text");
    assert_eq!(
        err_of(&fixture("depth-bomb.bin")),
        ErrorKind::DepthLimit,
        "the one error this crate can produce"
    );
}

#[test]
fn the_browser_fragment_keeps_the_styling_it_states_inline() {
    let runs = styled(&fixture("browser-fragment.bin"));
    assert!(runs.iter().any(|(t, s)| t == "brown" && s.bold));
    assert!(runs.iter().any(|(t, s)| t == "lazy" && s.italic));
    assert!(
        runs.iter()
            .all(|(_, s)| s.font_family.as_deref() == Some("Helvetica")),
        "the div's font-family is inherited by everything under it"
    );
    // And nothing from the <style> block, because there is no cascade.
    assert!(runs.iter().all(|(_, s)| s.color.is_none()));
}

#[test]
fn every_prefix_of_every_fixture_terminates_without_panicking() {
    for name in fixture_names() {
        let bytes = fixture(&name);
        for cut in 0..bytes.len() {
            let prefix = &bytes[..cut];
            let _ = Document::parse(prefix);
            for run in Runs::new(prefix) {
                if run.is_err() {
                    break;
                }
            }
            for token in Tokenizer::new(prefix) {
                if let Token::StartTag(tag) = token {
                    for attr in tag.attributes() {
                        let _ = attr.value.chars().count();
                    }
                }
            }
        }
    }
}

#[test]
fn mutated_fixtures_never_panic() {
    // A cheap stand-in for the fuzzer: flip bytes to the characters that
    // actually change the lexer's mind.
    for name in fixture_names() {
        let bytes = fixture(&name);
        for (i, byte) in [b'<', b'>', b'&', b'"', b'\'', b'/', b'=', b';', 0xff]
            .iter()
            .enumerate()
        {
            for step in 0..40 {
                let at = (step * 7 + i) % bytes.len().max(1);
                let mut mutated = bytes.clone();
                if let Some(slot) = mutated.get_mut(at) {
                    *slot = *byte;
                }
                let _ = Document::parse(&mutated);
            }
        }
    }
}

// ---------------------------------------------------------------- elements

#[test]
fn the_element_tables_agree_with_themselves() {
    assert!(element::is_void("BR") && element::is_void("img"));
    assert!(!element::is_void("div"));
    assert!(!element::is_block("br"), "a break is not a block boundary");
    assert!(element::is_block("P") && element::is_block("li"));
    assert!(element::drops_content("STYLE") && element::drops_content("script"));
    assert!(element::preserves_whitespace("pre"));
    assert!(element::formatting("STRONG").is_some());
    assert!(element::formatting("div").is_none());
}

#[test]
fn as_str_declines_when_pre_drops_its_leading_newline() {
    // Found by the `html_tokenize` fuzz target, which decodes every span both
    // ways and compares.
    //
    // HTML says a newline immediately after a `<pre>` start tag is ignored,
    // which is why `<pre>\ncode</pre>` renders without a blank first line.
    // `chars` implements that; the borrowed bytes still have the newline in
    // them, so they are *not* the decoded text and the fast path has to
    // decline rather than hand back a different answer.
    let t = HtmlText::new(b"\ncode", Whitespace::Preserve, true);
    assert_eq!(t.chars().collect::<String>(), "code");
    assert_eq!(
        t.as_str(),
        None,
        "the raw bytes still carry the newline that `chars` drops"
    );

    // Not at a boundary, the newline is content and the fast path is correct.
    let mid = HtmlText::new(b"\ncode", Whitespace::Preserve, false);
    assert_eq!(mid.as_str(), Some("\ncode"));
    assert_eq!(mid.chars().collect::<String>(), "\ncode");
}

#[test]
fn as_str_and_chars_never_disagree() {
    // The general property, and the one the fuzz target actually asserts: the
    // fast path is an optimisation, so whenever it answers it must give exactly
    // what the slow path would. A disagreement is worse than a slow path —
    // two APIs on one type returning different text for the same input.
    let raws: &[&[u8]] = &[
        b"",
        b"hello",
        b"\n",
        b"\ncode",
        b"\r\ncode",
        b"code\n",
        b"a  b",
        b"a&amp;b",
        b"&",
        b"\n\nfoo",
        b"\ttab",
        "caf\u{e9}".as_bytes(),
        b"\xFF\xFE",
    ];
    for raw in raws {
        for ws in [Whitespace::Collapse, Whitespace::Preserve] {
            for boundary in [true, false] {
                let t = HtmlText::new(raw, ws, boundary);
                if let Some(fast) = t.as_str() {
                    assert_eq!(
                        fast,
                        t.chars().collect::<String>(),
                        "fast path disagreed for {raw:?} ws={ws:?} boundary={boundary}"
                    );
                }
            }
        }
    }
}

//! The HTML lexer — `rclip_html::Tokenizer`, `Tag::attributes`, `HtmlText`.
//!
//! There is no signature and no error: every byte string is *some* HTML, which
//! is the whole reason a clipboard can hand you one. So this target has no
//! magic gate at all and every mutation reaches the state machine, which makes
//! it the cheapest of the five and the one that should find a hang first.
//!
//! What it is really guarding is **progress**. The crate's author found and
//! removed two self-recursions during review — `Attrs::next` recursed once per
//! `=` on `<a =====...>`, and `Tokenizer::next` recursed once per repetition on
//! `</></>...` — and replaced both with loops. A loop that fails to consume a
//! byte is the same bug wearing different clothes: instead of a stack overflow
//! you get an infinite loop, which libFuzzer reports as a timeout rather than a
//! crash and which is just as fatal to a paste. The assertions below are that
//! every yielded token strictly advanced the cursor and that the token count is
//! bounded by the input length, so neither failure can pass silently.
//!
//! Both shapes are in this target's corpus as `seed-recursion-*`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_html::{css, HtmlText, Token, Tokenizer, Whitespace};

/// Drive an `HtmlText` through both of its readers and check they agree.
///
/// `as_str` is the borrowing fast path and `chars` is the general one. It is
/// documented as conservative — `None` only means "go the long way" — so the
/// property is one-directional: whenever it says yes, the two must produce the
/// same characters. A fast path that disagrees with the slow one is a paste
/// whose text depends on which accessor the caller happened to use.
fn text_agrees(t: &HtmlText<'_>) {
    let decoded: String = t.chars().collect();
    if let Some(s) = t.as_str() {
        assert_eq!(s, decoded, "as_str disagreed with chars");
    }
    assert_eq!(t.is_empty(), decoded.is_empty());
}

fuzz_target!(|data: &[u8]| {
    // Both whitespace policies and both boundary states. `Whitespace::Preserve`
    // is not a flag the tokenizer merely stores: it changes which bytes
    // `HtmlChars` collapses and turns on the leading-newline strip, so it is a
    // second code path rather than a second constant.
    for (ws, boundary) in [
        (Whitespace::Collapse, true),
        (Whitespace::Collapse, false),
        (Whitespace::Preserve, true),
        (Whitespace::Preserve, false),
    ] {
        let mut tok = Tokenizer::new(data);
        tok.set_whitespace(ws, boundary);

        let mut last_pos = 0usize;
        let mut tokens = 0usize;
        // `while let` rather than `for`, because the cursor has to be read
        // between tokens and a `for` would hold the borrow across the body.
        #[allow(clippy::while_let_on_iterator)]
        while let Some(token) = tok.next() {
            tokens += 1;
            let pos = tok.pos();
            let start = tok.token_offset();

            // The progress property. Without it, a lexer that failed to consume
            // a byte would spin here forever and libFuzzer would report a
            // timeout with no indication of which loop it was in.
            assert!(pos > last_pos, "tokenizer yielded a token without consuming");
            assert!(pos <= data.len(), "cursor ran past the buffer");
            assert!(start < pos, "token_offset did not precede the cursor");
            assert!(
                tokens <= data.len(),
                "more tokens than input bytes: a token consumed nothing"
            );
            last_pos = pos;

            match token {
                Token::StartTag(tag) => {
                    // The attribute region is a slice of the tag, so it can
                    // never be longer than what the tag consumed.
                    assert!(tag.attrs.len() <= pos - start, "attrs outran the tag");
                    assert!(tag.offset < pos);

                    // `<a =====…>` and `<a ////…>` are the `Attrs::next`
                    // recursion shapes. Every `=` used to be one stack frame;
                    // now every iteration must eat at least one byte, and an
                    // attribute count above the region's length would mean it
                    // does not.
                    let mut attrs = 0usize;
                    for attr in tag.attributes() {
                        attrs += 1;
                        assert!(
                            attrs <= tag.attrs.len() + 1,
                            "more attributes than bytes to hold them"
                        );
                        // A name is a run of bytes taken from the region, so
                        // it cannot be longer than the region.
                        assert!(attr.name.len() <= tag.attrs.len());
                        text_agrees(&attr.value);
                    }

                    // The one attribute the crate reads for styling, through
                    // the CSS splitter, which does its own entity decoding and
                    // its own quote and paren tracking.
                    if let Some(style) = tag.attr("style") {
                        let mut decls = 0usize;
                        for decl in css::declarations(style.as_raw()) {
                            decls += 1;
                            assert!(
                                decls <= style.as_raw().len() + 1,
                                "more declarations than bytes"
                            );
                            let _ = decl.is("color");
                            let _ = css::font_weight_bold(decl.value);
                            let _ = css::font_style_italic(decl.value);
                            let _ = css::text_decoration(decl.value);
                            let _ = css::color(decl.value);
                            let _ = css::font_size_pt(decl.value, Some(css::DEFAULT_PT));
                        }
                    }
                    // The presentational attributes, which are a separate
                    // reader from `style=`.
                    for name in ["face", "color", "size", "bgcolor"] {
                        if let Some(v) = tag.attr(name) {
                            text_agrees(&v);
                            let _ = css::font_attr_size_pt(v.as_raw());
                            let _ = css::named_color(css::trim(v.as_raw()));
                        }
                    }
                    let _ = (tag.self_closing, tag.is("br"), tag.name);
                }
                Token::EndTag { name, offset } => {
                    assert!(offset < pos);
                    let _ = name;
                }
                Token::Text(t) => {
                    // `Token::Text` is documented as never empty *in bytes*.
                    assert!(!t.as_raw().is_empty(), "empty text token");
                    text_agrees(&t);
                }
                Token::Comment(body) | Token::Doctype(body) => {
                    assert!(body.len() <= pos - start, "body outran its token");
                }
            }
        }
        // Fused: once the input is exhausted the iterator must stay exhausted.
        assert!(tok.next().is_none(), "tokenizer restarted after the end");
        let _ = tok.in_raw_text();
    }
});

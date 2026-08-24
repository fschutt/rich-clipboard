//! The lexer: bytes in, [`Token`]s out, no state beyond a cursor and one flag.
//!
//! This is a tokenizer and not a parser. It has no idea whether `</b>` matches
//! anything, and that is deliberate: mismatched nesting is the *normal* case in
//! clipboard HTML, so the repair belongs one layer up where the element stack
//! lives, and the lexer's only job is to never lose sync.
//!
//! # What it is not
//!
//! Not a browser. It does not implement the WHATWG tokenizer's 80 states, does
//! not adjust foreign-content elements, does not do the "bogus comment" or
//! "character reference in attribute" states by name, and has no notion of an
//! insertion mode. See the crate README for the scope boundary.
//!
//! # The one piece of state
//!
//! [`Token::StartTag`] for `script`, `style`, `title`, `textarea`, `xmp`,
//! `iframe`, `noembed`, `noframes` and `noscript` puts the lexer into raw-text
//! mode until the matching end tag: everything between is one [`Token::Text`]
//! and a `<` inside it is not markup. Without this, the CSS in the `<style>`
//! block that every browser puts at the top of a clipboard fragment lexes as a
//! stream of tags and its selectors end up in the user's document.

use rclip_core::Reader;

use crate::text::{HtmlText, Whitespace};

/// One lexical item.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Token<'a> {
    /// `<div class="x">` or `<br/>`.
    StartTag(Tag<'a>),
    /// `</div>`.
    EndTag {
        /// The tag name, as written. Compare case-insensitively.
        name: &'a str,
        /// Byte offset of the `<`.
        offset: usize,
    },
    /// A run of character data. Never empty.
    Text(HtmlText<'a>),
    /// `<!-- ... -->`, the delimiters stripped.
    Comment(&'a [u8]),
    /// `<!DOCTYPE html>` or any other `<!...>` that is not a comment, and
    /// `<?...>`, which HTML treats as a bogus comment.
    Doctype(&'a [u8]),
}

/// An opening tag and its attribute region.
///
/// The attributes are *not* parsed here. Most tags carry none that matter, and
/// scanning `class`, `id` and `data-*` on every element to find the one `style`
/// attribute is work with nothing at the end of it. [`Tag::attrs`] does it on
/// demand.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Tag<'a> {
    /// The tag name, as written. Compare case-insensitively; use [`Tag::is`].
    pub name: &'a str,
    /// Everything between the name and the closing `>`, undecoded.
    pub attrs: &'a [u8],
    /// `true` for `<br/>`. Meaningless in HTML for anything but a foreign
    /// element, and recorded rather than acted on.
    pub self_closing: bool,
    /// Byte offset of the `<`.
    pub offset: usize,
}

impl<'a> Tag<'a> {
    /// `true` if this is the named element, ASCII-case-insensitively.
    #[must_use]
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    /// Iterate the attributes.
    #[must_use]
    pub fn attributes(&self) -> Attrs<'a> {
        Attrs {
            r: Reader::new(self.attrs),
        }
    }

    /// The first attribute with this name, ASCII-case-insensitively.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<HtmlText<'a>> {
        self.attributes()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .map(|a| a.value)
    }
}

/// One `name="value"` pair.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Attr<'a> {
    /// The name, as written.
    pub name: &'a str,
    /// The value. Empty for a boolean attribute, which is indistinguishable
    /// from `attr=""` and means the same thing everywhere it matters here.
    pub value: HtmlText<'a>,
}

/// Iterator over a tag's attributes.
///
/// Accepts all three quoting forms — `a="b"`, `a='b'`, `a=b` — because all
/// three appear in clipboard markup and refusing one loses the styling it
/// carried.
#[derive(Debug, Clone)]
pub struct Attrs<'a> {
    r: Reader<'a>,
}

impl<'a> Iterator for Attrs<'a> {
    type Item = Attr<'a>;

    fn next(&mut self) -> Option<Attr<'a>> {
        // A loop and not recursion: `<a ==========...>` is a valid byte string
        // and one `self.next()` per `=` would put the stack depth under the
        // control of the input.
        let name_len = loop {
            skip_ws(&mut self.r);
            let rest = self.r.remaining();
            // A name runs to whitespace, `=`, `/` or `>`. A leading `/` is the
            // stray slash of `<br />` and is not a name.
            let len = rest
                .iter()
                .take_while(|b| !matches!(b, b'=' | b'/' | b'>') && !b.is_ascii_whitespace())
                .count();
            if len > 0 {
                break len;
            }
            // Not a name: skip one byte so a malformed region cannot stand
            // still, and try again until the region is exhausted.
            self.r.skip(1).ok()?;
        };
        let start = self.r.pos();
        let name = as_str(self.r.take(name_len).ok()?);
        skip_ws(&mut self.r);
        if self.r.remaining().first() != Some(&b'=') {
            return Some(Attr {
                name,
                value: HtmlText::new(&[], Whitespace::Preserve, false),
            });
        }
        self.r.skip(1).ok()?;
        skip_ws(&mut self.r);

        let quote = match self.r.remaining().first() {
            Some(q @ (b'"' | b'\'')) => Some(*q),
            _ => None,
        };
        let raw = match quote {
            Some(q) => {
                self.r.skip(1).ok()?;
                let rest = self.r.remaining();
                let len = rest.iter().take_while(|b| **b != q).count();
                let value = self.r.take(len).ok()?;
                // The closing quote, if the tag was not truncated.
                let _ = self.r.skip(1);
                value
            }
            None => {
                let rest = self.r.remaining();
                let len = rest
                    .iter()
                    .take_while(|b| !b.is_ascii_whitespace() && **b != b'>')
                    .count();
                self.r.take(len).ok()?
            }
        };
        debug_assert!(self.r.pos() > start, "attribute scan must consume");
        Some(Attr {
            name,
            // Attribute values are not whitespace-collapsed: `style="color:
            // red"` and `font-family: 'Foo Bar'` both depend on the spacing
            // they were written with, and the CSS splitter does its own
            // trimming.
            value: HtmlText::new(raw, Whitespace::Preserve, false),
        })
    }
}

/// Pull lexer over an HTML fragment.
///
/// Never fails: there is no byte sequence that is not *some* HTML, and a
/// clipboard payload that a tokenizer refused would be a paste that did
/// nothing. Structural problems surface one layer up, as
/// [`rclip_core::ErrorKind::DepthLimit`] and nowhere else.
#[derive(Debug, Clone)]
pub struct Tokenizer<'a> {
    r: Reader<'a>,
    token_start: usize,
    /// The element whose raw text is being consumed, if any.
    raw_until: Option<&'a str>,
    /// Whether the *next* text run may begin with collapsible whitespace.
    /// Owned by the caller through [`Tokenizer::set_boundary`].
    at_boundary: bool,
    /// Whitespace handling for the next text run.
    ws: Whitespace,
}

impl<'a> Tokenizer<'a> {
    /// Start lexing.
    #[must_use]
    pub const fn new(input: &'a [u8]) -> Self {
        Self {
            r: Reader::new(input),
            token_start: 0,
            raw_until: None,
            at_boundary: true,
            ws: Whitespace::Collapse,
        }
    }

    /// Byte offset of the token most recently returned.
    #[must_use]
    pub const fn token_offset(&self) -> usize {
        self.token_start
    }

    /// Byte offset just past it.
    #[must_use]
    pub const fn pos(&self) -> usize {
        self.r.pos()
    }

    /// Tell the lexer how to treat whitespace in the text runs that follow.
    ///
    /// The element stack knows whether it is inside `<pre>` and the lexer does
    /// not, so this is set from outside. `at_boundary` says whether a leading
    /// run of whitespace should vanish entirely — true at the start of the
    /// document and directly after a line break, where a browser drops it.
    pub fn set_whitespace(&mut self, ws: Whitespace, at_boundary: bool) {
        self.ws = ws;
        self.at_boundary = at_boundary;
    }

    /// `true` if the lexer is inside a raw-text element.
    #[must_use]
    pub const fn in_raw_text(&self) -> bool {
        self.raw_until.is_some()
    }

    /// Consume the raw text of a `<script>` / `<style>` / ... up to its end tag.
    fn raw_text(&mut self, name: &'a str) -> Token<'a> {
        let rest = self.r.remaining();
        let end = find_end_tag(rest, name).unwrap_or(rest.len());
        let text = self.r.take(end).unwrap_or_default();
        self.raw_until = None;
        Token::Text(HtmlText::new(text, Whitespace::Preserve, false))
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        // A loop and not recursion. `<style></style><style></style>...` and
        // `</></></>...` are both byte strings a hostile producer can write,
        // and one `self.next()` per repetition would put the stack depth under
        // the control of the input.
        loop {
            self.token_start = self.r.pos();
            if self.r.remaining_len() == 0 {
                return None;
            }
            if let Some(name) = self.raw_until {
                // An empty raw-text region (`<style></style>`) yields no token,
                // so clear the mode and carry on rather than returning an empty
                // run.
                let token = self.raw_text(name);
                if let Token::Text(t) = token {
                    if !t.is_empty_raw() {
                        return Some(token);
                    }
                }
                continue;
            }

            let rest = self.r.remaining();
            if rest.first() == Some(&b'<') {
                match self.markup() {
                    Lexed::Token(token) => return Some(token),
                    // `</>` and `<!-- -->` produce nothing at all.
                    Lexed::Nothing => continue,
                    // A `<` that begins nothing — `a < b` — is text. Fall
                    // through, and make sure the run starts *after* it so the
                    // scan below does not stop on the same byte forever.
                    Lexed::Text => {}
                }
            }

            // Character data, up to the next `<`.
            let rest = self.r.remaining();
            let skip = usize::from(rest.first() == Some(&b'<'));
            let len = rest
                .get(skip..)
                .unwrap_or_default()
                .iter()
                .take_while(|b| **b != b'<')
                .count()
                + skip;
            let raw = self.r.take(len).unwrap_or_default();
            let text = HtmlText::new(raw, self.ws, self.at_boundary);
            if text.is_empty_raw() {
                return None;
            }
            return Some(Token::Text(text));
        }
    }
}

/// What lexing a `<` produced.
enum Lexed<'a> {
    /// A token.
    Token(Token<'a>),
    /// Something that was markup and carries nothing — `</>`.
    Nothing,
    /// Not markup at all: `a < b`, which is prose.
    Text,
}

impl<'a> Tokenizer<'a> {
    /// Lex a `<`.
    fn markup(&mut self) -> Lexed<'a> {
        let offset = self.r.pos();
        let rest = self.r.remaining();
        match rest.get(1) {
            Some(b'!') => return Lexed::Token(self.bang(offset)),
            Some(b'?') => {
                // A processing instruction is a bogus comment in HTML.
                let len = rest.iter().take_while(|b| **b != b'>').count();
                let Ok(body) = self.r.take(len) else {
                    return Lexed::Text;
                };
                let _ = self.r.skip(1);
                return Lexed::Token(Token::Doctype(body));
            }
            Some(b'/') => {
                let name_len = tag_name_len(rest.get(2..).unwrap_or_default());
                if name_len == 0 {
                    // `</>` is not an end tag; a browser drops it.
                    let len = rest.iter().take_while(|b| **b != b'>').count() + 1;
                    let _ = self.r.skip(len.min(self.r.remaining_len()));
                    return Lexed::Nothing;
                }
                let _ = self.r.skip(2);
                let name = self.r.take(name_len).map(as_str).unwrap_or("");
                self.skip_to_gt();
                return Lexed::Token(Token::EndTag { name, offset });
            }
            Some(b) if b.is_ascii_alphabetic() => {}
            // `<3`, `< b`, or a `<` at the very end: text.
            _ => return Lexed::Text,
        }

        let name_len = tag_name_len(rest.get(1..).unwrap_or_default());
        let _ = self.r.skip(1);
        let name = self.r.take(name_len).map(as_str).unwrap_or("");

        // The attribute region runs to the `>` that is not inside a quoted
        // value. Quoting matters: `<a title="x>y">` is one tag.
        let rest = self.r.remaining();
        let attrs_len = attrs_len(rest);
        let attrs = self.r.take(attrs_len).unwrap_or_default();
        let _ = self.r.skip(1);

        let self_closing = attrs.last() == Some(&b'/');
        let attrs = if self_closing {
            attrs.get(..attrs.len() - 1).unwrap_or_default()
        } else {
            attrs
        };

        if is_raw_text(name) {
            self.raw_until = Some(name);
        }
        Lexed::Token(Token::StartTag(Tag {
            name,
            attrs,
            self_closing,
            offset,
        }))
    }

    /// `<!...`: a comment, a CDATA section, or a doctype.
    fn bang(&mut self, _offset: usize) -> Token<'a> {
        let rest = self.r.remaining();
        if rest.starts_with(b"<!--") {
            let body = rest.get(4..).unwrap_or_default();
            let end = find(body, b"-->");
            let len = end.unwrap_or(body.len());
            let _ = self.r.skip(4);
            let comment = self.r.take(len).unwrap_or_default();
            let _ = self.r.skip(3);
            return Token::Comment(comment);
        }
        // `<![CDATA[...]]>` is a bogus comment in HTML, but its *content* is
        // text in XHTML, which is what a few mail clients still emit.
        if rest.starts_with(b"<![CDATA[") {
            let body = rest.get(9..).unwrap_or_default();
            let len = find(body, b"]]>").unwrap_or(body.len());
            let _ = self.r.skip(9);
            let text = self.r.take(len).unwrap_or_default();
            let _ = self.r.skip(3);
            return Token::Text(HtmlText::new(text, self.ws, self.at_boundary));
        }
        let len = rest.iter().take_while(|b| **b != b'>').count();
        let body = self.r.take(len).unwrap_or_default();
        let _ = self.r.skip(1);
        Token::Doctype(body)
    }

    fn skip_to_gt(&mut self) {
        let len = self
            .r
            .remaining()
            .iter()
            .take_while(|b| **b != b'>')
            .count();
        let _ = self.r.skip(len);
        let _ = self.r.skip(1);
    }
}

// --------------------------------------------------------------- helpers

fn skip_ws(r: &mut Reader<'_>) {
    let n = r
        .remaining()
        .iter()
        .take_while(|b| b.is_ascii_whitespace())
        .count();
    let _ = r.skip(n);
}

/// Bytes of a tag name: ASCII letters, digits, `-` and `_`, which covers the
/// custom-element and namespaced (`o:p`, from Word) spellings that turn up.
fn tag_name_len(rest: &[u8]) -> usize {
    rest.iter()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':'))
        .count()
}

/// Length of the attribute region: up to the first `>` outside a quoted value.
fn attrs_len(rest: &[u8]) -> usize {
    let mut quote: Option<u8> = None;
    for (i, &b) in rest.iter().enumerate() {
        match (quote, b) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => return i,
            (None, _) => {}
        }
    }
    rest.len()
}

/// Position of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Offset of `</name` in `hay`, case-insensitively.
fn find_end_tag(hay: &[u8], name: &str) -> Option<usize> {
    let n = name.len();
    let mut i = 0;
    while i + 2 + n <= hay.len() {
        if hay[i] == b'<' && hay[i + 1] == b'/' {
            let candidate = hay.get(i + 2..i + 2 + n)?;
            let after = hay.get(i + 2 + n).copied().unwrap_or(b'>');
            if candidate.eq_ignore_ascii_case(name.as_bytes())
                && (after == b'>' || after.is_ascii_whitespace() || after == b'/')
            {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Elements whose content is character data rather than markup.
fn is_raw_text(name: &str) -> bool {
    [
        "script", "style", "title", "textarea", "xmp", "iframe", "noembed", "noframes", "noscript",
    ]
    .iter()
    .any(|k| name.eq_ignore_ascii_case(k))
}

/// A tag or attribute name as `&str`.
///
/// Names are ASCII in every document that has one; anything else is not a name
/// and comparing it against `"b"` will simply not match, so the empty string is
/// the right answer rather than an error that would cost the whole paste.
fn as_str(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("")
}

//! Enough of an XML scanner to pull the top-level `<key>`/`<string>` pairs out
//! of a property list.
//!
//! Not an XML parser and not trying to be. A `.webloc` is a fixed shape —
//! prolog, doctype, `<plist>`, one `<dict>` of string pairs — and the job is to
//! read one value out of it without pulling in a parser and a serde stack. What
//! this does handle, because getting it wrong would be silently wrong:
//!
//! - **Entity references in values.** CoreFoundation writes `&` as `&amp;`, so
//!   any URL with two query parameters is escaped. Decoding is deferred to
//!   [`Text`], which resolves entities while iterating rather than allocating.
//! - **Nesting.** Only pairs directly inside the outermost `<dict>` count, so a
//!   `URL` key buried inside a nested dictionary is not mistaken for the real
//!   one.
//! - **Quoted `>` inside attributes**, which would otherwise end a tag early.
//!
//! Entity *definitions* are a different matter: this scanner skips the doctype
//! without reading it and resolves only the five predefined entities plus
//! numeric references. A custom entity is an error rather than an expansion,
//! which is what makes the classic billion-laughs input a parse failure here
//! instead of an out-of-memory.

use rclip_core::{Error, ErrorKind, Result, MAX_DEPTH};

use crate::text::{has_entities, Text};

/// `true` if the buffer looks like an XML document — a `<` after optional
/// whitespace and an optional UTF-8 byte-order mark.
#[must_use]
pub fn detect(buf: &[u8]) -> bool {
    let rest = buf.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(buf);
    rest.iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|&b| b == b'<')
}

/// Strip a UTF-8 byte-order mark and validate the document as UTF-8.
///
/// XML plists are UTF-8 in practice; CoreFoundation has never written one in
/// anything else. Anything that is not gets rejected rather than transcoded.
pub fn as_str(buf: &[u8]) -> Result<&str> {
    let rest = buf.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(buf);
    core::str::from_utf8(rest).map_err(|e| Error::new(ErrorKind::InvalidUtf8, e.valid_up_to()))
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Tag<'a> {
    Open(&'a str),
    Close(&'a str),
    /// `<foo/>`.
    Empty(&'a str),
    /// A prolog, comment or doctype: skipped without being interpreted.
    Ignored,
}

/// Cursor over the document, yielding one tag at a time.
#[derive(Debug, Clone)]
struct Scanner<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Scanner<'a> {
    const fn new(s: &'a str) -> Self {
        Self { s, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        self.s.get(self.pos..).unwrap_or("")
    }

    /// Next tag, or `None` at the end of the document.
    fn next_tag(&mut self) -> Option<Result<Tag<'a>>> {
        let rest = self.rest();
        let lt = rest.find('<')?;
        let at = self.pos + lt;
        let after = &rest[lt + 1..];

        if let Some(body) = after.strip_prefix("?") {
            return Some(self.skip_to(body, "?>", at).map(|()| Tag::Ignored));
        }
        if let Some(body) = after.strip_prefix("!--") {
            return Some(self.skip_to(body, "-->", at).map(|()| Tag::Ignored));
        }
        if let Some(body) = after.strip_prefix("!") {
            // A doctype. Skipped whole — its contents are never interpreted, so
            // an internal subset defining entities has no effect on anything.
            return Some(self.skip_to(body, ">", at).map(|()| Tag::Ignored));
        }

        let closing = after.starts_with('/');
        let name_start = if closing { 1 } else { 0 };
        let body = &after[name_start..];
        let name_len = body
            .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
            .unwrap_or(body.len());
        let name = &body[..name_len];
        if name.is_empty() {
            return Some(Err(Error::new(ErrorKind::Malformed, at)));
        }

        // Find the closing '>', stepping over quoted attribute values so that a
        // '>' inside one does not end the tag early.
        let tail = &body[name_len..];
        let Some(gt) = find_tag_end(tail) else {
            return Some(Err(Error::new(ErrorKind::UnexpectedEof, at)));
        };
        let self_closing = tail[..gt].trim_end().ends_with('/');
        self.pos = at + 1 + name_start + name_len + gt + 1;

        Some(Ok(if closing {
            Tag::Close(name)
        } else if self_closing {
            Tag::Empty(name)
        } else {
            Tag::Open(name)
        }))
    }

    /// Advance past `needle`, starting from `body` (which begins at the current
    /// `<` plus a prefix).
    fn skip_to(&mut self, body: &str, needle: &str, at: usize) -> Result<()> {
        let body_start = self.s.len() - body.len();
        match body.find(needle) {
            Some(n) => {
                self.pos = body_start + n + needle.len();
                Ok(())
            }
            None => Err(Error::new(ErrorKind::UnexpectedEof, at)),
        }
    }

    /// Character data up to the next `<`, then consume `</name>`.
    ///
    /// Rejects anything other than the matching close tag. That rules out
    /// mixed content and `<![CDATA[`, neither of which appears in a property
    /// list, and means the returned slice is the whole value rather than the
    /// first fragment of it.
    fn text_until_close(&mut self, name: &str) -> Result<&'a str> {
        let start = self.pos;
        let rest = self.rest();
        let lt = rest
            .find('<')
            .ok_or(Error::new(ErrorKind::UnexpectedEof, start))?;
        let text = &rest[..lt];
        self.pos = start + lt;
        match self.next_tag() {
            Some(Ok(Tag::Close(n))) if n == name => Ok(text),
            Some(Err(e)) => Err(e),
            // Still unimplemented, and still deliberately: CoreFoundation's
            // XML writer escapes with entity references and never emits a
            // CDATA section. Checked in phase 4 by handing `plutil -convert
            // xml1` a string containing `&`, `<`, `>`, a quote and a literal
            // `]]>`; every one came back as an entity. Accepting CDATA would
            // add an unexercised branch to a parser whose input is written by
            // another process, which is the wrong trade until a real capture
            // needs it.
            _ => Err(Error::new(ErrorKind::Unsupported, start + lt)),
        }
    }
}

/// Find the `>` that ends a tag, skipping over single- and double-quoted runs.
fn find_tag_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut quote: Option<u8> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'>' => return Some(i),
                _ => {}
            },
        }
    }
    None
}

/// Wrap a raw value slice, marking it as needing entity decoding if it has any.
fn text(raw: &str) -> Text<'_> {
    if has_entities(raw) {
        Text::Escaped(raw)
    } else {
        Text::Utf8(raw)
    }
}

/// Iterator over the `<key>`/`<string>` pairs of the outermost `<dict>`.
///
/// Pairs inside a nested dictionary or array are skipped: a `URL` key one level
/// down is not the document's URL, and treating it as one would let a crafted
/// file redirect a reader that only looks for the first match.
#[derive(Debug, Clone)]
pub struct Entries<'a> {
    scanner: Scanner<'a>,
    depth: u32,
    pending_key: Option<Text<'a>>,
    done: bool,
}

impl<'a> Entries<'a> {
    #[must_use]
    pub const fn new(doc: &'a str) -> Self {
        Self {
            scanner: Scanner::new(doc),
            depth: 0,
            pending_key: None,
            done: false,
        }
    }

    fn step(&mut self) -> Option<Result<(Text<'a>, Text<'a>)>> {
        loop {
            let tag = match self.scanner.next_tag()? {
                Ok(t) => t,
                Err(e) => return Some(Err(e)),
            };
            match tag {
                Tag::Ignored => {}
                Tag::Open("dict" | "array") => {
                    self.depth += 1;
                    // Nesting is not recursion here, but an unbounded counter on
                    // attacker input is still a number worth capping.
                    if self.depth > MAX_DEPTH {
                        return Some(Err(Error::new(ErrorKind::DepthLimit, self.scanner.pos)));
                    }
                    self.pending_key = None;
                }
                Tag::Close("dict" | "array") => {
                    self.depth = self.depth.saturating_sub(1);
                    self.pending_key = None;
                }
                Tag::Open(name) if self.depth == 1 => {
                    let raw = match self.scanner.text_until_close(name) {
                        Ok(r) => r,
                        Err(e) => return Some(Err(e)),
                    };
                    match name {
                        "key" => self.pending_key = Some(text(raw)),
                        "string" => {
                            if let Some(k) = self.pending_key.take() {
                                return Some(Ok((k, text(raw))));
                            }
                        }
                        // <integer>, <date>, <data> … : a value this crate has
                        // no use for. The key it belonged to is dropped so it
                        // cannot pair with the next value instead.
                        _ => self.pending_key = None,
                    }
                }
                Tag::Empty(name) if self.depth == 1 => {
                    if let (Some(k), "string") = (self.pending_key.take(), name) {
                        return Some(Ok((k, Text::Utf8(""))));
                    }
                }
                Tag::Open(_) | Tag::Close(_) | Tag::Empty(_) => {}
            }
        }
    }
}

impl<'a> Iterator for Entries<'a> {
    type Item = Result<(Text<'a>, Text<'a>)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let out = self.step();
        if matches!(out, Some(Err(_)) | None) {
            self.done = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
	<dict>
	<key>URL</key>
	<string>https://example.com/a?x=1&amp;y=2</string>
	<key>URLName</key>
	<string>Example</string>
	</dict>
</plist>
"#;

    /// Collect the pairs as `(key, value)` predicates, since asserting without
    /// `alloc` means comparing rather than materialising.
    fn assert_pairs(doc: &str, expected: &[(&str, &str)]) {
        let mut n = 0;
        for (i, pair) in Entries::new(doc).enumerate() {
            let (k, v) = pair.expect("well-formed document");
            let (ek, ev) = expected[i];
            assert!(k.eq_str(ek), "key {i} should be {ek:?}");
            assert!(v.eq_str(ev), "value {i} should be {ev:?}");
            n += 1;
        }
        assert_eq!(n, expected.len(), "wrong number of pairs in {doc:?}");
    }

    #[test]
    fn reads_the_pairs_and_decodes_entities() {
        assert_pairs(
            DOC,
            &[
                ("URL", "https://example.com/a?x=1&y=2"),
                ("URLName", "Example"),
            ],
        );
    }

    #[test]
    fn nested_dictionaries_are_not_top_level() {
        // A `URL` key one level down must not be mistaken for the document's,
        // or a crafted file can redirect a reader that takes the first match.
        let doc = "<plist><dict><key>Inner</key><dict><key>URL</key><string>evil</string></dict>\
                   <key>URL</key><string>good</string></dict></plist>";
        assert_pairs(doc, &[("URL", "good")]);
    }

    #[test]
    fn non_string_values_do_not_steal_the_next_key() {
        let doc = "<plist><dict><key>Count</key><integer>3</integer>\
                   <key>URL</key><string>ok</string></dict></plist>";
        assert_pairs(doc, &[("URL", "ok")]);
    }

    #[test]
    fn self_closing_string_is_an_empty_value() {
        assert_pairs(
            "<plist><dict><key>URL</key><string/></dict></plist>",
            &[("URL", "")],
        );
    }

    #[test]
    fn attribute_containing_a_gt_does_not_end_the_tag() {
        let doc = "<plist version=\"1.0>\"><dict><key>URL</key><string>ok</string></dict></plist>";
        assert_pairs(doc, &[("URL", "ok")]);
    }

    #[test]
    fn unterminated_comment_is_an_error() {
        let doc = "<plist><dict><!-- never closed <key>URL</key><string>x</string></dict></plist>";
        assert_eq!(
            Entries::new(doc).next().unwrap().unwrap_err().kind,
            ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn detects_xml_behind_whitespace_and_a_bom() {
        assert!(detect(b"<?xml"));
        assert!(detect(b"\xEF\xBB\xBF  \n<plist>"));
        assert!(!detect(b"bplist00"));
        assert!(!detect(b""));
    }
}

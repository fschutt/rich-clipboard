//! Character data, decoded lazily.
//!
//! Text in HTML is not a slice of the document anywhere: `&amp;` is one
//! character written as five bytes, and a run of newlines and indentation
//! between two tags is one space. So this is a *view* rather than a `&str`,
//! with the same shape `rclip-rtf` uses for the same reason —
//! [`HtmlText::as_str`] for the fast path when the span happens to need
//! nothing done to it, and [`HtmlText::chars`] which always works.
//!
//! # Whitespace
//!
//! Collapsing is not cosmetic. A fragment copied out of a browser is pretty-
//! printed: every tag is on its own line and indented, and a reader that took
//! the text nodes literally would paste a document with a newline and four
//! spaces between every two words. CSS `white-space` is what really decides
//! this and is far out of scope; what is implemented is the default (collapse)
//! and `<pre>`/`<textarea>` (preserve), which is the distinction that changes
//! the output.

use rclip_core::Reader;

use crate::entity;

/// How a run of ASCII whitespace is treated.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum Whitespace {
    /// The HTML default: any run of whitespace is one space, and a run at the
    /// start of a line or directly after a break disappears.
    #[default]
    Collapse,
    /// `<pre>`, `<textarea>`: newlines and runs of spaces are content. CR and
    /// CRLF still normalize to LF, which is the one thing the HTML input stream
    /// does unconditionally.
    Preserve,
}

/// A span of character data.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct HtmlText<'a> {
    raw: &'a [u8],
    ws: Whitespace,
    /// `true` if a leading run of whitespace should vanish rather than become a
    /// space — the start of the document, or directly after a break.
    at_boundary: bool,
}

impl<'a> HtmlText<'a> {
    /// Wrap a raw span.
    #[must_use]
    pub const fn new(raw: &'a [u8], ws: Whitespace, at_boundary: bool) -> Self {
        Self {
            raw,
            ws,
            at_boundary,
        }
    }

    /// The undecoded bytes.
    #[must_use]
    pub const fn as_raw(&self) -> &'a [u8] {
        self.raw
    }

    /// How whitespace in this span is treated.
    #[must_use]
    pub const fn whitespace(&self) -> Whitespace {
        self.ws
    }

    /// The span as a borrowed string, when decoding it would change nothing.
    ///
    /// `None` does not mean the span is invalid — it means the caller has to go
    /// through [`HtmlText::chars`]. Deliberately conservative: it says yes only
    /// for valid UTF-8 with no `&` and, in collapsing mode, no whitespace at
    /// all, because those are the conditions under which the answer is
    /// obviously right.
    #[must_use]
    pub fn as_str(&self) -> Option<&'a str> {
        if self.raw.contains(&b'&') {
            return None;
        }
        if self.ws == Whitespace::Collapse && self.raw.iter().any(u8::is_ascii_whitespace) {
            return None;
        }
        if self.ws == Whitespace::Preserve && self.raw.contains(&b'\r') {
            return None;
        }
        // Preserving mode at a boundary drops one leading newline -- "a newline
        // immediately after a `<pre>` start tag is ignored", which is why
        // `<pre>\ncode</pre>` does not render with a blank first line. `chars`
        // does it through `strip_newline`; the borrowed bytes still have the
        // newline in them, so they are not the decoded text and this fast path
        // has to decline. Found by the `html_tokenize` fuzz target, which
        // decodes every span both ways and compares.
        if self.ws == Whitespace::Preserve
            && self.at_boundary
            && matches!(self.raw.first(), Some(b'\r' | b'\n'))
        {
            return None;
        }
        core::str::from_utf8(self.raw).ok()
    }

    /// Decode: character references resolved, whitespace handled, invalid UTF-8
    /// replaced with U+FFFD rather than refused.
    #[must_use]
    pub fn chars(&self) -> HtmlChars<'a> {
        HtmlChars {
            r: Reader::new(self.raw),
            ws: self.ws,
            prev_space: self.at_boundary,
            // "A newline immediately after a `<pre>` start tag is ignored" —
            // which is the whole reason `<pre>\ncode\n</pre>` does not render
            // with a blank first line. `at_boundary` is set by the block break
            // the `<pre>` itself produced, so this is the same flag doing the
            // preserving-mode version of the same job.
            strip_newline: self.ws == Whitespace::Preserve && self.at_boundary,
        }
    }

    /// `true` if this span decodes to nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chars().next().is_none()
    }

    /// `true` if this span ends with a newline that was preserved, so a block
    /// boundary right after it would be a second one.
    pub(crate) fn ends_with_newline(&self) -> bool {
        self.ws == Whitespace::Preserve && matches!(self.raw.last(), Some(b'\n' | b'\r'))
    }

    /// `true` if the *bytes* are empty, without decoding.
    pub(crate) const fn is_empty_raw(&self) -> bool {
        self.raw.is_empty()
    }

    /// What `at_boundary` should be for the span that follows this one.
    ///
    /// A span that emitted nothing leaves the boundary where it was; a span
    /// that ended in whitespace has already emitted the one space that run is
    /// worth, so the next one must not emit a second.
    pub(crate) fn next_boundary(&self, prev: bool) -> bool {
        let Some(last) = self.raw.last() else {
            return prev;
        };
        if self.ws == Whitespace::Preserve {
            // Nothing collapses here, so nothing is owed to the next span. The
            // boundary flag in preserving mode means only "strip one leading
            // newline", and that is set by a block break rather than by the
            // span before it.
            let _ = last;
            return false;
        }
        if self.raw.iter().all(u8::is_ascii_whitespace) {
            // All whitespace: either it collapsed to one space, or it was
            // dropped at a boundary and nothing changed.
            return if self.at_boundary { prev } else { true };
        }
        last.is_ascii_whitespace()
    }
}

/// Iterator over the decoded characters of an [`HtmlText`].
#[derive(Debug, Clone)]
pub struct HtmlChars<'a> {
    r: Reader<'a>,
    ws: Whitespace,
    /// Whether the previous character emitted was a collapsed space, so a
    /// second run of whitespace emits nothing.
    prev_space: bool,
    /// One leading newline still to be dropped, in preserving mode.
    strip_newline: bool,
}

impl Iterator for HtmlChars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        if self.strip_newline {
            self.strip_newline = false;
            let rest = self.r.remaining();
            let skip = match (rest.first(), rest.get(1)) {
                (Some(b'\r'), Some(b'\n')) => 2,
                (Some(b'\r' | b'\n'), _) => 1,
                _ => 0,
            };
            let _ = self.r.skip(skip);
        }
        loop {
            let at = self.r.pos();
            let b = *self.r.remaining().first()?;

            if b == b'&' {
                if let Some(reference) = entity::decode(self.r.buffer(), at) {
                    self.r.skip(reference.len).ok()?;
                    // A referenced character is content even when it is a
                    // space: the author wrote `&#32;` rather than a space
                    // precisely because they wanted one that survives.
                    self.prev_space = false;
                    return Some(reference.ch);
                }
                // Not a reference. `a & b` is three tokens and one of them is
                // an ampersand, which is what a browser does with it too.
                self.r.skip(1).ok()?;
                self.prev_space = false;
                return Some('&');
            }

            if b.is_ascii_whitespace() {
                if self.ws == Whitespace::Collapse {
                    let run = self
                        .r
                        .remaining()
                        .iter()
                        .take_while(|b| b.is_ascii_whitespace())
                        .count();
                    self.r.skip(run).ok()?;
                    if self.prev_space {
                        continue;
                    }
                    self.prev_space = true;
                    return Some(' ');
                }
                // Preserving. CR and CRLF both become LF, which is the one
                // normalization the HTML input stream always does.
                self.r.skip(1).ok()?;
                if b == b'\r' {
                    if self.r.remaining().first() == Some(&b'\n') {
                        self.r.skip(1).ok()?;
                    }
                    return Some('\n');
                }
                return Some(b as char);
            }

            self.prev_space = false;
            if b.is_ascii() {
                self.r.skip(1).ok()?;
                return Some(b as char);
            }
            return Some(self.utf8());
        }
    }
}

impl HtmlChars<'_> {
    /// Decode one non-ASCII character at the cursor, lossily.
    fn utf8(&mut self) -> char {
        let lead = self.r.remaining().first().copied().unwrap_or(0);
        let width = match lead {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            // A continuation byte or an invalid lead: one byte, one U+FFFD.
            _ => {
                let _ = self.r.skip(1);
                return char::REPLACEMENT_CHARACTER;
            }
        };
        let at = self.r.pos();
        // Through the reader, because `width` is derived from a byte that came
        // out of the input and `buf[at..at + width]` on a truncated sequence is
        // exactly the panic this workspace's rule 3 exists to prevent.
        match Reader::new(self.r.buffer())
            .slice_at(at, width)
            .ok()
            .and_then(|s| core::str::from_utf8(s).ok())
            .and_then(|s| s.chars().next())
        {
            Some(c) => {
                let _ = self.r.skip(width);
                c
            }
            None => {
                let _ = self.r.skip(1);
                char::REPLACEMENT_CHARACTER
            }
        }
    }
}

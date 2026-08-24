//! Line splitting that keeps the byte offset of every line.
//!
//! Splits on CRLF, LF **or** a lone CR. RFC 2483 §5 says "as for all text/*
//! formats, lines are terminated with a CRLF pair", and every real reader is
//! laxer than that:
//!
//! - GLib's `g_uri_list_extract_uris`, which is what GTK deserializes with:
//!   *"We also allow LF delimination as well as the specified CRLF."*
//! - Qt's `QMimeData::dataToUrls` splits on `'\n'` alone.
//! - Chromium's `URIListToFileInfos` passes `"\r\n"` to `SplitStringPiece`,
//!   which treats it as a *character set*, so any mixture works.
//!
//! Every parse error in this crate carries the offset it happened at, and the
//! only way to have one after `str::lines()` has thrown the position away is to
//! carry it along. Hence this instead of the standard iterator.

/// One physical line, without its terminator.
#[derive(Debug, Copy, Clone)]
pub(crate) struct Line<'a> {
    /// The line text, terminator excluded.
    pub text: &'a str,
    /// Byte offset of the first character of the line, relative to the buffer
    /// handed to the parser.
    pub offset: usize,
}

/// Splits on `\r\n`, `\n` or `\r`.
#[derive(Debug, Copy, Clone)]
pub(crate) struct Lines<'a> {
    rest: &'a str,
    offset: usize,
}

impl<'a> Lines<'a> {
    pub(crate) const fn new(src: &'a str, offset: usize) -> Self {
        Self { rest: src, offset }
    }

    /// The not-yet-consumed tail.
    pub(crate) const fn rest(&self) -> &'a str {
        self.rest
    }

    /// Byte offset of the next line to be produced.
    pub(crate) const fn offset(&self) -> usize {
        self.offset
    }
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Line<'a>> {
        if self.rest.is_empty() {
            return None;
        }
        let offset = self.offset;
        let (text, consumed) = match self.rest.find(['\r', '\n']) {
            Some(i) => {
                let text = &self.rest[..i];
                // A CR immediately followed by LF is one terminator, not two;
                // treating it as two would inject an empty line between every
                // pair of URIs in a spec-conforming CRLF list.
                let term = if self.rest.as_bytes().get(i) == Some(&b'\r')
                    && self.rest.as_bytes().get(i + 1) == Some(&b'\n')
                {
                    2
                } else {
                    1
                };
                (text, i + term)
            }
            None => (self.rest, self.rest.len()),
        };
        self.rest = &self.rest[consumed..];
        self.offset += consumed;
        Some(Line { text, offset })
    }
}

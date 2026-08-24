//! Line splitting that keeps the byte offset of every line.
//!
//! Every parse error in this crate carries the offset it happened at, and the
//! only way to have one after `str::lines()` has thrown the position away is to
//! carry it along. Hence this instead of the standard iterator.

/// One physical line, without its terminator.
#[derive(Debug, Copy, Clone)]
pub(crate) struct Line<'a> {
    /// The line text, with any trailing `\r` already removed.
    pub text: &'a str,
    /// Byte offset of the first character of the line, relative to the buffer
    /// handed to the parser (BOM included, so offsets match the file on disk).
    pub offset: usize,
}

/// Splits on `\n`, tolerating `\r\n`.
///
/// `.url` files are specified to use CRLF and real ones do, but a file that has
/// been through a Unix text tool will have bare LF and must still parse — the
/// terminator is not load-bearing for the format.
#[derive(Debug, Copy, Clone)]
pub(crate) struct Lines<'a> {
    rest: &'a str,
    offset: usize,
}

impl<'a> Lines<'a> {
    pub(crate) const fn new(src: &'a str, offset: usize) -> Self {
        Self { rest: src, offset }
    }

    /// The not-yet-consumed tail. Used to carve a section body out of the
    /// source by length difference rather than by index arithmetic.
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
        let (raw, consumed) = match self.rest.find('\n') {
            Some(i) => (&self.rest[..i], i + 1),
            None => (self.rest, self.rest.len()),
        };
        self.rest = &self.rest[consumed..];
        self.offset += consumed;
        Some(Line {
            text: raw.strip_suffix('\r').unwrap_or(raw),
            offset,
        })
    }
}

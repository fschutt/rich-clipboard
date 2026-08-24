//! `text/uri-list` — how files actually move on Linux.
//!
//! Two things live here, because in practice they are never used apart:
//!
//! - **RFC 2483 §5** `text/uri-list`: CRLF-separated URIs, lines beginning `#`
//!   are comments, everything percent-encoded. This module.
//! - **The cut-vs-copy conventions** that RFC 2483 has no room for —
//!   `x-special/gnome-copied-files`, `application/x-kde-cutselection` and the
//!   legacy Nautilus text payload. See [`convention`], which cites the source
//!   of each grammar, since none of them is specified anywhere. Without one of
//!   them every paste of files reads as a copy.
//!
//! [`emit::RECOMMENDED`] answers the question a clipboard source actually has:
//! *which of these do I publish?* (All of them; they do not read each other's.)
//!
//! # Borrowing and `alloc`
//!
//! Parsing borrows. Percent-*validating* a URI and iterating its decoded bytes
//! work with no allocator; producing the decoded string does not, so
//! [`Uri::to_decoded_bytes`] and the serializers in [`emit`] are behind the
//! `alloc` feature.
//!
//! # Example
//!
//! ```
//! # use rclip_uri_list::{convention::{parse_copied_files, FileAction}};
//! let payload = b"cut\nfile:///home/me/a%20file.txt\nfile:///home/me/b.txt";
//! let cf = parse_copied_files(payload).unwrap();
//! assert_eq!(cf.action(), FileAction::Cut);
//!
//! let first = cf.uris().next().unwrap();
//! assert_eq!(first.as_file().unwrap().path(), "/home/me/a%20file.txt");
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

pub mod convention;
pub mod emit;
mod lines;
pub mod shortcut;
pub mod uri;

use rclip_core::{Error, ErrorKind, Reader, Result};

use lines::Lines;

pub use convention::FileAction;
pub use shortcut::ShortcutTarget;
pub use uri::{FileUri, PercentDecode, Uri};

/// A parsed `text/uri-list`.
///
/// Borrows the caller's buffer; no URI is decoded until asked for.
#[derive(Debug, Copy, Clone)]
pub struct UriList<'a> {
    src: &'a str,
    /// Offset of `src` within the original buffer, so reported offsets match a
    /// hex dump of the payload even after a BOM was skipped.
    base: usize,
}

/// Parse a `text/uri-list`.
///
/// # Errors
///
/// [`ErrorKind::InvalidUtf8`] if the bytes are not UTF-8. RFC 2483 registers
/// `charset` as an optional parameter and says URIs "can be represented using
/// US-ASCII"; in practice every producer sends UTF-8 or pure ASCII, and a
/// non-UTF-8 payload is a bug worth surfacing rather than guessing at.
pub fn parse(bytes: &[u8]) -> Result<UriList<'_>> {
    let (src, base) = decode(bytes)?;
    Ok(UriList { src, base })
}

/// Validate as UTF-8, skip a BOM, and drop one trailing NUL.
///
/// The NUL is not hypothetical. Qt carries a comment about it in
/// `qmimedata.cpp`: *"Qt 3.x will send text/uri-list with a trailing
/// null-terminator (that is not sent for any other text/\* mime-type), so chop
/// it off"*. Left in place it becomes a trailing empty line at best, and part
/// of the last filename at worst.
fn decode(bytes: &[u8]) -> Result<(&str, usize)> {
    let mut r = Reader::new(bytes);
    let base = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        r.skip(3)?;
        3
    } else {
        0
    };
    let mut rest = r.remaining();
    if let Some(head) = rest.strip_suffix(&[0u8]) {
        rest = head;
    }
    let src = core::str::from_utf8(rest)
        .map_err(|e| Error::new(ErrorKind::InvalidUtf8, base + e.valid_up_to()))?;
    Ok((src, base))
}

/// One meaningful line of a list. Blank lines are not entries.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Entry<'a> {
    /// A comment line. RFC 2483 §5: "Any lines beginning with the `#`
    /// character are comment lines and are ignored during processing. (Note
    /// that URIs may contain the `#` character, so it is only a comment
    /// character when it is the first character on a line.)"
    Comment {
        /// The text after the `#`, untrimmed.
        text: &'a str,
        /// Byte offset of the `#`.
        offset: usize,
    },
    /// A URI line.
    Uri(Uri<'a>),
}

impl<'a> UriList<'a> {
    /// The list text, BOM and trailing NUL excluded.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        self.src
    }

    pub(crate) const fn raw_lines(&self) -> Lines<'a> {
        Lines::new(self.src, self.base)
    }

    /// The remainder of the list after `lines` has consumed some of it.
    ///
    /// Used by [`convention`] to hand back the URIs that follow a verb line
    /// without re-scanning or copying.
    pub(crate) const fn tail_of(&self, lines: &Lines<'a>) -> Self {
        Self { src: lines.rest(), base: lines.offset() }
    }

    /// Every comment and URI, in order.
    #[must_use]
    pub const fn entries(&self) -> Entries<'a> {
        Entries { lines: self.raw_lines() }
    }

    /// Just the URIs.
    #[must_use]
    pub const fn uris(&self) -> Uris<'a> {
        Uris { entries: self.entries() }
    }

    /// The first URI, which for a mapped resolution RFC 2483 says is preceded
    /// by a comment naming the original.
    #[must_use]
    pub fn first(&self) -> Option<Uri<'a>> {
        self.uris().next()
    }

    /// Check every URI's percent-encoding in one pass.
    ///
    /// # Errors
    ///
    /// The first [`Uri::validate_percent_encoding`] failure.
    pub fn validate_percent_encoding(&self) -> Result<()> {
        for uri in self.uris() {
            uri.validate_percent_encoding()?;
        }
        Ok(())
    }
}

/// Iterator over the entries of a [`UriList`].
#[derive(Debug, Copy, Clone)]
pub struct Entries<'a> {
    lines: Lines<'a>,
}

impl<'a> Iterator for Entries<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Entry<'a>> {
        for line in self.lines.by_ref() {
            // Trim before classifying. RFC 2483 does not allow surrounding
            // whitespace, but GLib's `g_uri_list_extract_uris` — which is what
            // GTK deserializes with — documents that it "trims whitespace off
            // the ends", and Qt's reader calls `.trimmed()`. Not trimming would
            // make this the only reader on the desktop that chokes on a stray
            // space, and a raw space in a URI is invalid anyway, so nothing
            // meaningful is being discarded.
            let trimmed = line.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let offset = line.offset + (line.text.len() - line.text.trim_start().len());
            if let Some(text) = trimmed.strip_prefix('#') {
                return Some(Entry::Comment { text, offset });
            }
            return Some(Entry::Uri(Uri::new(trimmed, offset)));
        }
        None
    }
}

/// Iterator over just the URIs of a [`UriList`].
#[derive(Debug, Copy, Clone)]
pub struct Uris<'a> {
    entries: Entries<'a>,
}

impl<'a> Iterator for Uris<'a> {
    type Item = Uri<'a>;

    fn next(&mut self) -> Option<Uri<'a>> {
        loop {
            match self.entries.next()? {
                Entry::Uri(u) => return Some(u),
                Entry::Comment { .. } => {}
            }
        }
    }
}

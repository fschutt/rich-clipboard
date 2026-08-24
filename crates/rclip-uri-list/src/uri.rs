//! One URI from a list, and the percent-encoding around it.
//!
//! Percent-*validation* and iteration are `no_std` with no allocator: a
//! decoded URI is shorter than the encoded one but it is still a new string,
//! so decoding lives behind the `alloc` feature and everything else does not.

use rclip_core::{Error, ErrorKind, Result};

use crate::shortcut::{self, ShortcutTarget};

/// A single URI line.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Uri<'a> {
    raw: &'a str,
    offset: usize,
}

impl<'a> Uri<'a> {
    pub(crate) const fn new(raw: &'a str, offset: usize) -> Self {
        Self { raw, offset }
    }

    /// The URI exactly as it appeared, still percent-encoded.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        self.raw
    }

    /// Byte offset of the URI in the input buffer.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// The RFC 3986 scheme, lowercase or not, as written.
    #[must_use]
    pub fn scheme(&self) -> Option<&'a str> {
        shortcut::scheme(self.raw)
    }

    /// `true` if the scheme is `file`, compared case-insensitively.
    ///
    /// RFC 3986 §3.1 says schemes are case-insensitive and "should be
    /// normalized to lowercase"; producers mostly do, and one that does not
    /// still means the same thing.
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.scheme().is_some_and(|s| s.eq_ignore_ascii_case("file"))
    }

    /// Split a `file:` URI into authority and path.
    ///
    /// Returns `None` for any other scheme. The path is still percent-encoded
    /// and keeps its leading `/`.
    #[must_use]
    pub fn as_file(&self) -> Option<FileUri<'a>> {
        if !self.is_file() {
            return None;
        }
        let rest = self.raw.get(self.scheme()?.len() + 1..)?;
        // `file:///path` — empty authority — is what GTK, KIO and Chromium all
        // emit. `file://host/path` and the authority-less `file:/path` both
        // occur in hand-written input and mean the same thing to a reader.
        let (host, path) = match rest.strip_prefix("//") {
            Some(after) => match after.find('/') {
                Some(i) => (after.get(..i)?, after.get(i..)?),
                // `file://host` with no path at all.
                None => (after, ""),
            },
            None => ("", rest),
        };
        Some(FileUri { host, path })
    }

    /// Where this URI points.
    #[must_use]
    pub fn target(&self) -> ShortcutTarget<'a> {
        ShortcutTarget::classify(self.raw)
    }

    /// Check that every `%` introduces two hex digits.
    ///
    /// Worth doing before handing a URI to anything else: a lone `%` means the
    /// producer built the string by concatenation instead of by encoding, and
    /// whatever follows it is not what it appears to be. Costs one pass and no
    /// allocation.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] at the offset of the offending `%`.
    pub fn validate_percent_encoding(&self) -> Result<()> {
        let b = self.raw.as_bytes();
        let mut i = 0usize;
        while i < b.len() {
            if b[i] == b'%' {
                let ok = b
                    .get(i + 1..i + 3)
                    .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit));
                if !ok {
                    return Err(Error::new(ErrorKind::Malformed, self.offset + i));
                }
                i += 3;
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// The decoded bytes, one at a time, without allocating.
    ///
    /// Yields bytes rather than `char`s on purpose: a percent-encoded POSIX
    /// path is a byte string, and it is not required to be UTF-8. Decoding to
    /// `char` would have to either fail or corrupt such a path.
    #[must_use]
    pub const fn percent_decode(&self) -> PercentDecode<'a> {
        PercentDecode { rest: self.raw.as_bytes(), offset: self.offset }
    }
}

/// A `file:` URI split into its parts.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct FileUri<'a> {
    host: &'a str,
    path: &'a str,
}

impl<'a> FileUri<'a> {
    /// The authority. Empty for `file:///…`; `localhost` is equivalent to
    /// empty per RFC 8089 §3.
    #[must_use]
    pub const fn host(&self) -> &'a str {
        self.host
    }

    /// `true` if the URI names a file on this machine.
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.host.is_empty() || self.host.eq_ignore_ascii_case("localhost")
    }

    /// The path, still percent-encoded, with its leading `/`.
    ///
    /// Not decoded, not normalized, not resolved. A `text/uri-list` on the
    /// clipboard is written by another process, and `%2e%2e%2f` decodes to
    /// `../` — whoever turns this into a filesystem operation has to be the one
    /// deciding what that means.
    #[must_use]
    pub const fn path(&self) -> &'a str {
        self.path
    }
}

/// Iterator over the percent-decoded bytes of a URI.
#[derive(Debug, Copy, Clone)]
pub struct PercentDecode<'a> {
    rest: &'a [u8],
    offset: usize,
}

impl Iterator for PercentDecode<'_> {
    type Item = Result<u8>;

    fn next(&mut self) -> Option<Result<u8>> {
        let (&first, tail) = self.rest.split_first()?;
        if first != b'%' {
            self.rest = tail;
            self.offset += 1;
            return Some(Ok(first));
        }
        let at = self.offset;
        let Some(pair) = self.rest.get(1..3) else {
            self.rest = &[];
            return Some(Err(Error::new(ErrorKind::UnexpectedEof, at)));
        };
        let (Some(hi), Some(lo)) = (hex(pair[0]), hex(pair[1])) else {
            self.rest = &[];
            return Some(Err(Error::new(ErrorKind::Malformed, at)));
        };
        self.rest = self.rest.get(3..).unwrap_or(&[]);
        self.offset += 3;
        Some(Ok(hi << 4 | lo))
    }
}

const fn hex(b: u8) -> Option<u8> {
    Some(match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => return None,
    })
}

#[cfg(feature = "alloc")]
mod with_alloc {
    extern crate alloc;

    use alloc::{string::String, vec::Vec};
    use rclip_core::{Error, ErrorKind, Result};

    use super::Uri;

    impl Uri<'_> {
        /// Percent-decode into owned bytes.
        ///
        /// # Errors
        ///
        /// [`ErrorKind::Malformed`] or [`ErrorKind::UnexpectedEof`] for a
        /// truncated or non-hex escape.
        pub fn to_decoded_bytes(&self) -> Result<Vec<u8>> {
            let mut out = Vec::with_capacity(self.as_str().len());
            for b in self.percent_decode() {
                out.push(b?);
            }
            Ok(out)
        }

        /// Percent-decode and require the result to be UTF-8.
        ///
        /// Use [`Uri::to_decoded_bytes`] for a POSIX path: filenames on Linux
        /// are byte strings and the ones that are not UTF-8 are exactly the
        /// ones a caller most needs to handle rather than reject.
        ///
        /// # Errors
        ///
        /// As [`Uri::to_decoded_bytes`], plus [`ErrorKind::InvalidUtf8`].
        pub fn to_decoded_string(&self) -> Result<String> {
            let bytes = self.to_decoded_bytes()?;
            String::from_utf8(bytes)
                .map_err(|e| Error::new(ErrorKind::InvalidUtf8, self.offset() + e.utf8_error().valid_up_to()))
        }
    }
}

//! One URI from a list, and the percent-encoding around it, both directions.
//!
//! Percent-*validation* and iteration are `no_std` with no allocator: a
//! decoded URI is shorter than the encoded one but it is still a new string,
//! so producing one lives behind the `alloc` feature and everything else does
//! not. The encoder is the same shape — [`percent_encode`] is an iterator of
//! ASCII bytes that also implements [`Display`](core::fmt::Display), so a
//! `no_std` caller can write straight into a `fmt::Write` and never allocate.

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
        self.scheme()
            .is_some_and(|s| s.eq_ignore_ascii_case("file"))
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
        PercentDecode {
            rest: self.raw.as_bytes(),
            offset: self.offset,
        }
    }
}

/// Which bytes a percent-encoder may leave alone.
///
/// Getting this set wrong is the whole difficulty, and it breaks in both
/// directions. **Under-encoding** loses data: an unescaped `#` in a filename
/// turns the rest of the path into a URI fragment, so `notes#2.txt` arrives as
/// `notes`. **Over-encoding** breaks interoperation: `%2F` is not a path
/// separator, so escaping `/` turns one path into one long segment, and readers
/// that compare URIs textually — RFC 3986 §6.2.2.2 is explicit that a
/// percent-encoded *reserved* character is not equivalent to its literal form —
/// stop matching URIs they produced themselves.
///
/// The sets below are the RFC 3986 productions, not an invented compromise.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EncodeSet {
    /// A whole `path` inside a `file:` URI: `pchar` plus `/`.
    ///
    /// `pchar = unreserved / pct-encoded / sub-delims / ":" / "@"` (§3.3), and
    /// `/` is the segment separator, so all of
    /// `A-Za-z0-9-._~!$&'()*+,;=:@/` stay literal.
    ///
    /// It is *close to* what GLib produces, and differs by exactly one byte.
    /// Measured against GLib 2.88.3 by sweeping every printable ASCII byte
    /// through `g_filename_to_uri`:
    ///
    /// ```text
    /// GLib leaves literal:  ! $ & ' ( ) * + , - . : = @ ~ 0-9 A-Z a-z _ /
    /// GLib escapes:         " # % ; < > ? [ \ ] ^ ` { | }
    /// ```
    ///
    /// So GLib escapes `;` as `%3B` and this set does not. `g_filename_to_uri`
    /// does not use `G_URI_RESERVED_CHARS_ALLOWED_IN_PATH` — it escapes against
    /// a more conservative *unsafe* list — so a claim of byte-for-byte identity
    /// would be wrong.
    ///
    /// Both forms decode to the same path, so this is an interop nit rather
    /// than corruption: the only thing that can notice is a receiver comparing
    /// URI *text* instead of decoded paths, for a filename containing a
    /// semicolon. If that ever turns up in practice, the fix is a separate
    /// GLib-exact variant rather than bending this one — `EncodeSet` is
    /// `#[non_exhaustive]` precisely so that stays cheap. Bending `Path` would
    /// make it not `pchar`, which is the one thing its name promises.
    Path,
    /// One path *segment*: `pchar` only.
    ///
    /// `/` is escaped, so a filename that contains a slash cannot silently
    /// become two path components. Use this when encoding a single name;
    /// use [`EncodeSet::Path`] when encoding a path that already has structure.
    Segment,
    /// RFC 3986 `unreserved` only: `A-Za-z0-9-._~`.
    ///
    /// Escapes everything else, including the sub-delims and `/`. Maximally
    /// conservative and therefore *not* what a `file:` path wants — it is here
    /// for the places a URI component has to survive being embedded in
    /// something else.
    Unreserved,
}

impl EncodeSet {
    /// Whether `b` may appear literally.
    #[must_use]
    pub const fn allows(self, b: u8) -> bool {
        // `%` is never in any set: a literal `%` in a name must become `%25` or
        // the next reader takes the two bytes after it for an escape. It is not
        // in `unreserved` or `sub-delims`, so this falls out of the productions
        // rather than being a special case — but it is the one byte where
        // getting it wrong silently corrupts a filename, so it is worth naming.
        if b.is_ascii_alphanumeric() {
            return true;
        }
        match b {
            // unreserved
            b'-' | b'.' | b'_' | b'~' => true,
            // sub-delims, plus the two gen-delims `pchar` admits
            b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':'
            | b'@' => !matches!(self, Self::Unreserved),
            b'/' => matches!(self, Self::Path),
            _ => false,
        }
    }
}

/// Percent-encode `bytes`, leaving the members of `set` literal.
///
/// Takes bytes rather than a `&str` on purpose, and for the same reason
/// [`Uri::percent_decode`] yields them: a POSIX path is a byte string and is not
/// required to be UTF-8, and the paths that are not are exactly the ones a
/// caller most needs to move rather than reject.
///
/// The result is always ASCII, so it can be written straight into a `str` or a
/// `fmt::Write` — [`PercentEncode`] implements
/// [`Display`](core::fmt::Display) for that, and iterating it yields the
/// encoded bytes one at a time without allocating.
///
/// ```
/// use rclip_uri_list::uri::{percent_encode, EncodeSet};
///
/// // A space, a `#` and a literal `%`, all of which change meaning if left.
/// let encoded = percent_encode(b"/tmp/a file#2 100%.txt", EncodeSet::Path);
/// assert_eq!(encoded.to_string(), "/tmp/a%20file%232%20100%25.txt");
/// ```
#[must_use]
pub const fn percent_encode(bytes: &[u8], set: EncodeSet) -> PercentEncode<'_> {
    PercentEncode {
        rest: bytes,
        set,
        pending: 0,
        byte: 0,
    }
}

/// How many bytes [`percent_encode`] will produce.
///
/// For a caller writing into a fixed buffer with no allocator. Cannot overflow
/// in practice — the answer is at most three times the input — but it saturates
/// rather than wrapping if it ever could.
#[must_use]
pub fn encoded_len(bytes: &[u8], set: EncodeSet) -> usize {
    bytes.iter().fold(0usize, |acc, &b| {
        acc.saturating_add(if set.allows(b) { 1 } else { 3 })
    })
}

/// Iterator over the percent-encoded bytes of a byte string.
///
/// Every byte it yields is ASCII. Returned by [`percent_encode`].
#[derive(Debug, Copy, Clone)]
pub struct PercentEncode<'a> {
    rest: &'a [u8],
    set: EncodeSet,
    /// `2` = the two hex digits of `byte` are still owed, `1` = the low one is,
    /// `0` = read the next input byte.
    pending: u8,
    byte: u8,
}

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

impl Iterator for PercentEncode<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        // RFC 3986 §2.1: "should use uppercase letters for the digits". GLib,
        // Qt and Chromium all do, and a lowercase escape is a textual mismatch
        // for a reader that compares URIs without normalising them first.
        match self.pending {
            2 => {
                self.pending = 1;
                return Some(HEX_UPPER[usize::from(self.byte >> 4)]);
            }
            1 => {
                self.pending = 0;
                return Some(HEX_UPPER[usize::from(self.byte & 0x0F)]);
            }
            _ => {}
        }
        let (&b, tail) = self.rest.split_first()?;
        self.rest = tail;
        if self.set.allows(b) {
            Some(b)
        } else {
            self.byte = b;
            self.pending = 2;
            Some(b'%')
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.rest.len();
        let owed = usize::from(self.pending);
        (n + owed, n.checked_mul(3).map(|m| m + owed))
    }
}

impl core::iter::FusedIterator for PercentEncode<'_> {}

impl core::fmt::Display for PercentEncode<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use core::fmt::Write as _;

        // Every yielded byte is ASCII by construction — the allowed sets are
        // ASCII and an escape is `%` plus two hex digits — so the cast is a
        // widening of a code point below 0x80 and never a mojibake.
        for b in *self {
            f.write_char(char::from(b))?;
        }
        Ok(())
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

    use super::{percent_encode, EncodeSet, Uri};

    /// [`percent_encode`] into an owned `String`.
    ///
    /// The borrowed form implements `Display`, so this is only needed when the
    /// result has to outlive the input or be handed on as a `String`.
    #[must_use]
    pub fn percent_encode_to_string(bytes: &[u8], set: EncodeSet) -> String {
        let mut out = String::with_capacity(super::encoded_len(bytes, set));
        for b in percent_encode(bytes, set) {
            // ASCII by construction; see the `Display` impl.
            out.push(char::from(b));
        }
        out
    }

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
            String::from_utf8(bytes).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidUtf8,
                    self.offset() + e.utf8_error().valid_up_to(),
                )
            })
        }
    }
}

#[cfg(feature = "alloc")]
pub use with_alloc::percent_encode_to_string;

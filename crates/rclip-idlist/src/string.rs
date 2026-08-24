//! Strings that are either system-code-page bytes or UTF-16LE.
//!
//! Shell items — and, downstream, `.lnk` `StringData` — store the same logical
//! field in two encodings depending on which Windows wrote it. UTF-16LE decodes
//! anywhere; "the system default code page" only decodes on a machine that
//! knows which code page that was. That information is not in the payload, so
//! this type refuses to guess: it hands back ASCII for free and hands back raw
//! bytes otherwise, and a caller that knows the code page can decode them.

use core::fmt;

use rclip_core::{utf16::Utf16Le, Error, ErrorKind};

/// A borrowed string field out of a shell structure.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ShellStr<'a> {
    /// Bytes in the writer's system default code page, without a terminator.
    ///
    /// ASCII is a subset of every code page Windows ships, so bytes below 0x80
    /// are decodable; anything above is not, without external knowledge.
    Ansi(&'a [u8]),
    /// UTF-16LE code units, without a terminating NUL.
    Utf16(&'a [u8]),
}

impl<'a> ShellStr<'a> {
    /// The undecoded bytes, in whichever encoding this is.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        match self {
            Self::Ansi(b) | Self::Utf16(b) => b,
        }
    }

    #[must_use]
    pub const fn is_unicode(&self) -> bool {
        matches!(self, Self::Utf16(_))
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }

    /// Borrow as `&str` when that is free: an ANSI field whose bytes are all
    /// ASCII. Returns `None` for UTF-16 (which would need re-encoding, and so
    /// an allocation) and for ANSI holding a byte the code page decides.
    #[must_use]
    pub fn as_ascii(&self) -> Option<&'a str> {
        match self {
            Self::Ansi(b) if b.is_ascii() => core::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    /// Decode to `char`s.
    ///
    /// An ANSI byte at or above 0x80 yields [`ErrorKind::Unsupported`] — the
    /// bytes are well-formed, there is just no code page for them — and
    /// iteration continues, because a single-byte encoding cannot lose sync.
    /// A malformed UTF-16 sequence yields [`ErrorKind::InvalidUtf16`] and stops,
    /// because after a lone surrogate the rest of the field is not trustworthy.
    #[must_use]
    pub const fn chars(&self) -> ShellChars<'a> {
        match *self {
            Self::Ansi(b) => ShellChars::Ansi { bytes: b, pos: 0 },
            Self::Utf16(b) => ShellChars::Utf16(Utf16Le::new(b)),
        }
    }
}

impl fmt::Display for ShellStr<'_> {
    /// Best effort, substituting U+FFFD. Anything that must round-trip should
    /// go through [`ShellStr::as_bytes`] instead.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.chars() {
            fmt::Write::write_char(f, c.unwrap_or('\u{FFFD}'))?;
        }
        Ok(())
    }
}

/// Iterator over the `char`s of a [`ShellStr`].
#[derive(Debug, Clone)]
pub enum ShellChars<'a> {
    #[doc(hidden)]
    Ansi { bytes: &'a [u8], pos: usize },
    #[doc(hidden)]
    Utf16(Utf16Le<'a>),
}

impl Iterator for ShellChars<'_> {
    type Item = Result<char, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Ansi { bytes, pos } => {
                let b = *bytes.get(*pos)?;
                let at = *pos;
                *pos += 1;
                if b < 0x80 {
                    Some(Ok(char::from(b)))
                } else {
                    Some(Err(Error::new(ErrorKind::Unsupported, at)))
                }
            }
            Self::Utf16(it) => it.next(),
        }
    }
}

impl core::iter::FusedIterator for ShellChars<'_> {}

#[cfg(feature = "alloc")]
mod with_alloc {
    extern crate alloc;

    use alloc::string::String;

    use super::ShellStr;

    impl ShellStr<'_> {
        /// Decode, substituting U+FFFD for anything undecodable.
        ///
        /// For an ANSI field this means every byte at or above 0x80 becomes a
        /// replacement character. That is deliberate: silently pretending the
        /// code page was Latin-1 produces a path that looks right and is wrong,
        /// which is worse than one that is visibly lossy.
        ///
        /// A caller that knows the code page wants `to_string_lossy_with`
        /// instead, behind the `codepage` feature: it substitutes only where
        /// the named page is genuinely undefined rather than everywhere above
        /// `0x7F`.
        #[must_use]
        pub fn to_string_lossy(&self) -> String {
            let mut out = String::new();
            for c in self.chars() {
                out.push(c.unwrap_or('\u{FFFD}'));
            }
            out
        }
    }
}

#[cfg(feature = "codepage")]
mod with_codepage {
    #[cfg(feature = "alloc")]
    extern crate alloc;

    use rclip_codepage::{Decoder, Encoding};
    use rclip_core::{utf16::Utf16Le, Error};

    use super::ShellStr;

    impl<'a> ShellStr<'a> {
        /// Decode to `char`s, naming the code page the ANSI half is in.
        ///
        /// This is the answer to the question [`ShellStr::chars`] refuses to
        /// answer. The code page still is not in the payload — it never will
        /// be — but a caller that learned it from `CF_LOCALE`, from a `.lnk`
        /// `ConsoleFEDataBlock`, or from the user can now say so, and the
        /// bytes above `0x7F` stop being a hole.
        ///
        /// An undefined byte still yields an error rather than U+FFFD, and
        /// iteration continues past it: a single-byte code page cannot lose
        /// sync. `ansi` is ignored for a [`ShellStr::Utf16`] field.
        #[must_use]
        pub const fn chars_with(&self, ansi: Encoding) -> ShellCharsWith<'a> {
            match *self {
                Self::Ansi(b) => ShellCharsWith::Ansi(ansi.decode(b)),
                Self::Utf16(b) => ShellCharsWith::Utf16(Utf16Le::new(b)),
            }
        }

        /// Decode with a named code page, failing on anything undecodable.
        ///
        /// # Errors
        ///
        /// [`ErrorKind::Malformed`](rclip_core::ErrorKind::Malformed) at a byte
        /// the code page leaves undefined,
        /// [`ErrorKind::InvalidUtf16`](rclip_core::ErrorKind::InvalidUtf16) for
        /// a lone surrogate in a Unicode field. Offsets are into the field, not
        /// into the original payload.
        #[cfg(feature = "alloc")]
        pub fn to_string_with(&self, ansi: Encoding) -> rclip_core::Result<alloc::string::String> {
            let mut out = alloc::string::String::new();
            for c in self.chars_with(ansi) {
                out.push(c?);
            }
            Ok(out)
        }

        /// Decode with a named code page, substituting U+FFFD.
        ///
        /// Unlike [`ShellStr::to_string_lossy`], which replaces *every* byte
        /// above `0x7F`, this replaces only the ones the named code page leaves
        /// undefined.
        #[cfg(feature = "alloc")]
        #[must_use]
        pub fn to_string_lossy_with(&self, ansi: Encoding) -> alloc::string::String {
            let mut out = alloc::string::String::new();
            for c in self.chars_with(ansi) {
                out.push(c.unwrap_or('\u{FFFD}'));
            }
            out
        }
    }

    /// Iterator over the `char`s of a [`ShellStr`] decoded with a named code
    /// page. Returned by [`ShellStr::chars_with`].
    #[derive(Debug, Clone)]
    pub enum ShellCharsWith<'a> {
        #[doc(hidden)]
        Ansi(Decoder<'a>),
        #[doc(hidden)]
        Utf16(Utf16Le<'a>),
    }

    impl Iterator for ShellCharsWith<'_> {
        type Item = Result<char, Error>;

        fn next(&mut self) -> Option<Self::Item> {
            match self {
                Self::Ansi(it) => it.next(),
                Self::Utf16(it) => it.next(),
            }
        }
    }

    impl core::iter::FusedIterator for ShellCharsWith<'_> {}
}

#[cfg(feature = "codepage")]
pub use with_codepage::ShellCharsWith;

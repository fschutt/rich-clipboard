//! UTF-16LE decoding without `alloc`.
//!
//! Win32 clipboard structures are UTF-16LE throughout, and the payloads are
//! attacker-controlled, so lone surrogates are a case that happens rather than
//! a case that shouldn't. The iterator reports them instead of substituting
//! U+FFFD, and callers that want lossy behaviour opt in explicitly.

use crate::error::{Error, ErrorKind};

/// Iterator over the `char`s of a UTF-16LE byte slice.
///
/// Yields `Err` for a lone surrogate or a trailing odd byte, then stops.
#[derive(Debug, Clone)]
pub struct Utf16Le<'a> {
    bytes: &'a [u8],
    pos: usize,
    done: bool,
}

impl<'a> Utf16Le<'a> {
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0, done: false }
    }

    fn unit(&self, at: usize) -> Option<u16> {
        let b = self.bytes.get(at..at + 2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }
}

impl Iterator for Utf16Le<'_> {
    type Item = Result<char, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.pos >= self.bytes.len() {
            return None;
        }
        let at = self.pos;
        let Some(first) = self.unit(at) else {
            self.done = true;
            return Some(Err(Error::new(ErrorKind::InvalidUtf16, at)));
        };
        self.pos += 2;

        match first {
            // High surrogate: needs a matching low surrogate.
            0xD800..=0xDBFF => {
                let Some(second) = self.unit(self.pos) else {
                    self.done = true;
                    return Some(Err(Error::new(ErrorKind::InvalidUtf16, at)));
                };
                if !(0xDC00..=0xDFFF).contains(&second) {
                    self.done = true;
                    return Some(Err(Error::new(ErrorKind::InvalidUtf16, at)));
                }
                self.pos += 2;
                let cp = 0x1_0000
                    + ((u32::from(first) - 0xD800) << 10)
                    + (u32::from(second) - 0xDC00);
                match char::from_u32(cp) {
                    Some(c) => Some(Ok(c)),
                    None => {
                        self.done = true;
                        Some(Err(Error::new(ErrorKind::InvalidUtf16, at)))
                    }
                }
            }
            // Unpaired low surrogate.
            0xDC00..=0xDFFF => {
                self.done = true;
                Some(Err(Error::new(ErrorKind::InvalidUtf16, at)))
            }
            _ => match char::from_u32(u32::from(first)) {
                Some(c) => Some(Ok(c)),
                None => {
                    self.done = true;
                    Some(Err(Error::new(ErrorKind::InvalidUtf16, at)))
                }
            },
        }
    }
}

/// `true` if the slice decodes cleanly as UTF-16LE.
#[must_use]
pub fn is_valid_utf16le(bytes: &[u8]) -> bool {
    bytes.len() % 2 == 0 && Utf16Le::new(bytes).all(|r| r.is_ok())
}

/// Number of `char`s a UTF-16LE slice decodes to, or `None` if invalid.
#[must_use]
pub fn utf16le_char_count(bytes: &[u8]) -> Option<usize> {
    let mut n = 0usize;
    for c in Utf16Le::new(bytes) {
        c.ok()?;
        n += 1;
    }
    Some(n)
}

#[cfg(feature = "alloc")]
mod with_alloc {
    extern crate alloc;
    use alloc::string::String;

    use super::Utf16Le;

    /// Decode UTF-16LE, substituting U+FFFD for anything malformed.
    #[must_use]
    pub fn decode_utf16le_lossy(bytes: &[u8]) -> String {
        let mut out = String::new();
        for r in Utf16Le::new(bytes) {
            out.push(r.unwrap_or('\u{FFFD}'));
        }
        out
    }

    /// Encode to UTF-16LE bytes.
    #[must_use]
    pub fn encode_utf16le(s: &str) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec::Vec::with_capacity(s.len() * 2);
        for unit in s.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out
    }
}

#[cfg(feature = "alloc")]
pub use with_alloc::{decode_utf16le_lossy, encode_utf16le};

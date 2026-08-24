//! Iterators over the `char`s of a code page byte slice.
//!
//! Shaped to match [`rclip_core::utf16::Utf16Le`] so a caller can swap one for
//! the other, with one deliberate difference: UTF-16 *stops* on a lone
//! surrogate, because everything after it is suspect. A single-byte encoding
//! has no such coupling — byte *n* says nothing about byte *n+1* — so
//! [`Decoder`] reports an undefined byte and carries on.

use rclip_core::{Error, ErrorKind};

use crate::Encoding;

/// Iterator over the `char`s of a byte slice in a single-byte code page.
///
/// Exactly one item per input byte, whether or not that byte decodes. Yields
/// `Err(`[`ErrorKind::Malformed`]`)` at the offset of a byte the code page
/// leaves undefined.
#[derive(Debug, Clone)]
pub struct Decoder<'a> {
    enc: Encoding,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    /// Start decoding `bytes` as `enc`.
    #[must_use]
    pub const fn new(enc: Encoding, bytes: &'a [u8]) -> Self {
        Self { enc, bytes, pos: 0 }
    }

    /// The encoding being decoded with.
    #[must_use]
    pub const fn encoding(&self) -> Encoding {
        self.enc
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.pos.min(self.bytes.len())..]
    }

    /// Substitute U+FFFD for undefined bytes instead of reporting them.
    #[must_use]
    pub const fn lossy(self) -> LossyDecoder<'a> {
        LossyDecoder { inner: self }
    }
}

impl Iterator for Decoder<'_> {
    type Item = Result<char, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let b = *self.bytes.get(self.pos)?;
        let at = self.pos;
        self.pos += 1;
        match self.enc.decode_byte(b) {
            Some(c) => Some(Ok(c)),
            // Malformed rather than Unsupported: the byte is well formed and
            // this crate does implement the code page — the code page itself
            // assigns the value no character.
            None => Some(Err(Error::new(ErrorKind::Malformed, at))),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.bytes.len() - self.pos.min(self.bytes.len());
        (n, Some(n))
    }
}

impl ExactSizeIterator for Decoder<'_> {}
impl core::iter::FusedIterator for Decoder<'_> {}

/// Iterator over the `char`s of a byte slice, substituting U+FFFD.
///
/// Same one-`char`-per-byte guarantee as [`Decoder`]; the difference is only
/// what happens at an undefined byte.
#[derive(Debug, Clone)]
pub struct LossyDecoder<'a> {
    inner: Decoder<'a>,
}

impl<'a> LossyDecoder<'a> {
    /// Start decoding `bytes` as `enc`, substituting U+FFFD.
    #[must_use]
    pub const fn new(enc: Encoding, bytes: &'a [u8]) -> Self {
        Self {
            inner: Decoder::new(enc, bytes),
        }
    }

    /// The encoding being decoded with.
    #[must_use]
    pub const fn encoding(&self) -> Encoding {
        self.inner.encoding()
    }
}

impl Iterator for LossyDecoder<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        self.inner
            .next()
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for LossyDecoder<'_> {}
impl core::iter::FusedIterator for LossyDecoder<'_> {}

#[cfg(feature = "alloc")]
mod with_alloc {
    use alloc::{string::String, vec::Vec};

    use rclip_core::{Error, ErrorKind, Result};

    use crate::Encoding;

    impl Encoding {
        /// Decode a whole slice, failing on the first undefined byte.
        ///
        /// # Errors
        ///
        /// [`ErrorKind::Malformed`], carrying the offset of the byte this code
        /// page leaves undefined. Use [`Encoding::decode_to_string_lossy`] when
        /// a best-effort answer is wanted instead.
        pub fn decode_to_string(self, bytes: &[u8]) -> Result<String> {
            let mut out = String::with_capacity(bytes.len());
            for c in self.decode(bytes) {
                out.push(c?);
            }
            Ok(out)
        }

        /// Decode a whole slice, substituting U+FFFD for undefined bytes.
        #[must_use]
        pub fn decode_to_string_lossy(self, bytes: &[u8]) -> String {
            let mut out = String::with_capacity(bytes.len());
            out.extend(self.decode_lossy(bytes));
            out
        }

        /// Encode a string back to code page bytes.
        ///
        /// # Errors
        ///
        /// [`ErrorKind::Unsupported`], carrying the *byte* offset within `s` of
        /// the first character this code page cannot represent. Byte offset
        /// rather than character index because that is what slices `s` to show
        /// the caller what failed.
        pub fn encode_from_str(self, s: &str) -> Result<Vec<u8>> {
            let mut out = Vec::with_capacity(s.len());
            for (at, c) in s.char_indices() {
                match self.encode_char(c) {
                    Some(b) => out.push(b),
                    None => return Err(Error::new(ErrorKind::Unsupported, at)),
                }
            }
            Ok(out)
        }
    }
}

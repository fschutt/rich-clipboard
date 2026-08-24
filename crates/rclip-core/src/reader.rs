//! Bounds-checked cursor over a byte slice.
//!
//! Every codec in this workspace reads through this type rather than indexing
//! slices directly. That is not style preference: clipboard payloads are
//! attacker-controlled, and a raw `buf[off..off + len]` on a length field read
//! off the wire is the panic (or, with `get_unchecked`, the vulnerability) that
//! this type exists to make unreachable.
//!
//! All multi-byte integers are little-endian, because every format in scope
//! (Win32 structs, `BookmarkData`, DIB headers) is little-endian. The one
//! big-endian field in the workspace — `BookmarkData`'s date type — is read
//! with [`Reader::f64_be`].

use crate::error::{Error, ErrorKind, Result};

/// A forward cursor that cannot read out of bounds.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// The whole buffer, ignoring the cursor. Needed by formats whose offset
    /// fields are relative to the start of the structure rather than to the
    /// cursor — `LinkInfo`, `CIDA`, `CF_HTML`.
    #[must_use]
    pub const fn buffer(&self) -> &'a [u8] {
        self.buf
    }

    #[must_use]
    pub const fn pos(&self) -> usize {
        self.pos
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.buf.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> &'a [u8] {
        &self.buf[self.pos.min(self.buf.len())..]
    }

    #[must_use]
    pub const fn remaining_len(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// Error at the current position.
    #[must_use]
    pub const fn err(&self, kind: ErrorKind) -> Error {
        Error::new(kind, self.pos)
    }

    /// Move the cursor to an absolute offset. Fails rather than clamping, so a
    /// bad offset field surfaces where it is read instead of producing silently
    /// truncated output further down.
    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.buf.len() {
            return Err(Error::new(ErrorKind::BadOffset, pos));
        }
        self.pos = pos;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    /// Consume exactly `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| self.err(ErrorKind::TooLarge))?;
        if end > self.buf.len() {
            return Err(self.err(ErrorKind::UnexpectedEof));
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    /// A sub-reader over `n` bytes, advancing this cursor past them. Use for
    /// length-delimited records so an inner parser physically cannot read past
    /// its own record.
    pub fn take_reader(&mut self, n: usize) -> Result<Reader<'a>> {
        self.take(n).map(Reader::new)
    }

    /// Borrow `len` bytes at an absolute offset without moving the cursor.
    pub fn slice_at(&self, offset: usize, len: usize) -> Result<&'a [u8]> {
        let end = offset.checked_add(len).ok_or(Error::new(ErrorKind::TooLarge, offset))?;
        self.buf
            .get(offset..end)
            .ok_or(Error::new(ErrorKind::BadOffset, offset))
    }

    /// Everything from an absolute offset to the end of the buffer.
    pub fn tail_at(&self, offset: usize) -> Result<&'a [u8]> {
        self.buf.get(offset..).ok_or(Error::new(ErrorKind::BadOffset, offset))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        self.u8().map(|v| v as i8)
    }

    pub fn u16_le(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16_le(&mut self) -> Result<i16> {
        self.u16_le().map(|v| v as i16)
    }

    pub fn u32_le(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32_le(&mut self) -> Result<i32> {
        self.u32_le().map(|v| v as i32)
    }

    pub fn u64_le(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    pub fn i64_le(&mut self) -> Result<i64> {
        self.u64_le().map(|v| v as i64)
    }

    pub fn f64_le(&mut self) -> Result<f64> {
        self.u64_le().map(f64::from_bits)
    }

    /// Big-endian f64. Present for exactly one field in the workspace:
    /// `BookmarkData`'s `0x0400` date records, which are big-endian seconds
    /// since 2001-01-01 in an otherwise little-endian format.
    pub fn f64_be(&mut self) -> Result<f64> {
        let b = self.take(8)?;
        Ok(f64::from_bits(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ])))
    }

    /// Read a `u32` at an absolute offset without moving the cursor.
    pub fn peek_u32_le_at(&self, offset: usize) -> Result<u32> {
        let b = self.slice_at(offset, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a `u32` at the cursor without moving it.
    pub fn peek_u32_le(&self) -> Result<u32> {
        self.peek_u32_le_at(self.pos)
    }

    /// A 16-byte GUID in packet representation (MS-DTYP 2.3.4.2): three
    /// little-endian integers followed by eight raw bytes.
    pub fn guid(&mut self) -> Result<[u8; 16]> {
        let b = self.take(16)?;
        let mut out = [0u8; 16];
        out.copy_from_slice(b);
        Ok(out)
    }

    /// Consume bytes up to and including a single NUL, returning the bytes
    /// before it. Fails if no NUL is found before the end of the buffer.
    pub fn cstr_bytes(&mut self) -> Result<&'a [u8]> {
        let rest = self.remaining();
        match rest.iter().position(|&b| b == 0) {
            Some(n) => {
                let out = &rest[..n];
                self.pos += n + 1;
                Ok(out)
            }
            None => Err(self.err(ErrorKind::UnexpectedEof)),
        }
    }

    /// Consume a NUL-terminated ASCII/UTF-8 string.
    pub fn cstr_utf8(&mut self) -> Result<&'a str> {
        let at = self.pos;
        let bytes = self.cstr_bytes()?;
        core::str::from_utf8(bytes).map_err(|_| Error::new(ErrorKind::InvalidUtf8, at))
    }

    /// Consume bytes up to and including a UTF-16LE NUL unit, returning the
    /// bytes before it. The returned slice is still UTF-16LE — decode it with
    /// [`crate::utf16::Utf16Le`].
    pub fn utf16_nul_bytes(&mut self) -> Result<&'a [u8]> {
        let start = self.pos;
        let rest = self.remaining();
        let mut i = 0usize;
        while i + 1 < rest.len() {
            if rest[i] == 0 && rest[i + 1] == 0 {
                let out = &rest[..i];
                self.pos = start + i + 2;
                return Ok(out);
            }
            i += 2;
        }
        Err(self.err(ErrorKind::UnexpectedEof))
    }

    /// Fixed-width UTF-16LE field of `units` code units, truncated at the first
    /// NUL. Win32 structs are full of these — `FILEDESCRIPTORW::cFileName` is
    /// 260 units whether or not the name fills it.
    pub fn utf16_fixed(&mut self, units: usize) -> Result<&'a [u8]> {
        let raw = self.take(units.checked_mul(2).ok_or_else(|| self.err(ErrorKind::TooLarge))?)?;
        let mut end = raw.len();
        let mut i = 0usize;
        while i + 1 < raw.len() {
            if raw[i] == 0 && raw[i + 1] == 0 {
                end = i;
                break;
            }
            i += 2;
        }
        Ok(&raw[..end])
    }

    /// Reject a declared count that could not possibly be backed by the
    /// remaining input. `stride` is the minimum bytes one element occupies.
    ///
    /// This is the guard that keeps `Vec::with_capacity(header.count)` from
    /// being a one-line OOM: check before you trust.
    pub fn check_count(&self, count: usize, stride: usize) -> Result<()> {
        let need = count.checked_mul(stride).ok_or_else(|| self.err(ErrorKind::TooLarge))?;
        if need > self.remaining_len() {
            return Err(self.err(ErrorKind::TooLarge));
        }
        Ok(())
    }
}

impl<'a> From<&'a [u8]> for Reader<'a> {
    fn from(buf: &'a [u8]) -> Self {
        Self::new(buf)
    }
}

//! `CFSTR_FILEDESCRIPTORA` — the ANSI twin of `FILEGROUPDESCRIPTORW`.
//!
//! Same struct, one field narrower: `CHAR cFileName[260]` instead of
//! `WCHAR cFileName[260]`, so a descriptor is **332 bytes** rather than 592 and
//! the group is `cItems` followed by `cItems x 332`. Everything before the name
//! — flags, CLSID, icon size and position, attributes, three `FILETIME`s and the
//! two size `DWORD`s — is byte-for-byte identical, which is why both parsers
//! read that prefix through the same code.
//!
//! ```text
//! FILEGROUPDESCRIPTORA
//!   offset 0   cItems : UINT              number of descriptors
//!          4   fgd[]  : FILEDESCRIPTORA   cItems x 332 bytes
//!
//! FILEDESCRIPTORA                                             (332 bytes)
//!   offset   0   ... identical to FILEDESCRIPTORW through offset 72 ...
//!           72   cFileName[260]   : CHAR       260 bytes, NUL-truncated
//! ```
//!
//! # The code page is not in the payload
//!
//! `cFileName` is in the *writer's* system ANSI code page, and nothing in the
//! struct says which one that is. This is the same wall `CF_HDROP`'s
//! `fWide == 0` runs into, and it is answered the same way: [`FileDescriptorA`]
//! hands back the raw bytes and refuses to guess, and the optional, default-off
//! `codepage` feature adds [`FileDescriptorA::chars_with`] and friends, which
//! take the encoding as a parameter. A caller that knows — from `CF_LOCALE`,
//! from the transport, or from the user — says so; nothing here detects.
//!
//! # When you will actually see one
//!
//! Rarely, which is why this was deferred out of Phase 0. Every modern source
//! registers `"FileGroupDescriptorW"`. `"FileGroupDescriptor"` (no suffix) turns
//! up from old MFC and Delphi applications, from 16-bit-era shell extensions
//! still in service, and as the *fallback* format a data object offers
//! alongside the wide one — which is the case that matters, because a consumer
//! that negotiates formats by iterating `IEnumFORMATETC` can be handed it
//! first.

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use rclip_core::{Error, ErrorKind, Reader, Result};

use crate::{Flags, PointL, RawDescriptor, SizeL, GROUP_HEADER_LEN, OFF_FILENAME};

/// Size of one `FILEDESCRIPTORA`, in bytes: the same 72-byte prefix as the wide
/// form, then a 260-*byte* name field.
pub const DESCRIPTOR_A_LEN: usize = 332;

/// Capacity of the ANSI `cFileName`, in bytes. This is Win32 `MAX_PATH`.
pub const FILE_NAME_BYTES: usize = 260;

/// Longest name this crate will *write*: [`FILE_NAME_BYTES`] less the NUL.
///
/// Bytes, not characters. In a double-byte code page one character can cost two
/// of these, which is exactly why the writing API takes bytes and the
/// `codepage`-gated convenience encodes before it measures.
pub const MAX_WRITABLE_NAME_BYTES: usize = FILE_NAME_BYTES - 1;

const _: () = assert!(OFF_FILENAME + FILE_NAME_BYTES == DESCRIPTOR_A_LEN);

/// One `FILEDESCRIPTORA`.
///
/// Borrows the name from the input buffer; constructing one allocates nothing.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct FileDescriptorA<'a> {
    raw: RawDescriptor,
    /// `cFileName` truncated at its first NUL, still in the writer's code page.
    name: &'a [u8],
}

impl<'a> FileDescriptorA<'a> {
    /// Parse one descriptor from exactly [`DESCRIPTOR_A_LEN`] bytes.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::BadLength`] if `bytes` is not exactly 332 long. Every field
    /// is fixed-width, so nothing else here can fail.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() != DESCRIPTOR_A_LEN {
            return Err(Error::new(ErrorKind::BadLength, 0));
        }
        let mut r = Reader::new(bytes);
        let raw = RawDescriptor::parse_fixed(&mut r)?;
        debug_assert_eq!(r.pos(), OFF_FILENAME);
        // Fixed 260-byte field: the name is whatever precedes the first NUL,
        // and the rest is padding that must not leak into it.
        let field = r.take(FILE_NAME_BYTES)?;
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        let name = field
            .get(..end)
            .ok_or(Error::new(ErrorKind::BadLength, 0))?;
        Ok(Self { raw, name })
    }

    /// Every field verbatim, flags not applied.
    #[must_use]
    pub const fn raw(&self) -> RawDescriptor {
        self.raw
    }

    /// `dwFlags`.
    #[must_use]
    pub const fn flags(&self) -> Flags {
        self.raw.flags
    }

    /// `cFileName` as raw bytes, NUL and trailing padding removed.
    ///
    /// **Not decoded.** These are bytes in a code page this struct does not
    /// name; see the module documentation. As with the wide form, real producers
    /// put a *relative path* in here when they describe a folder tree —
    /// `sub\file.txt`, backslash-separated — and this crate resolves nothing.
    #[must_use]
    pub const fn file_name_ansi(&self) -> &'a [u8] {
        self.name
    }

    /// `clsid`, or `None` unless `FD_CLSID` is set.
    #[must_use]
    pub const fn clsid(&self) -> Option<[u8; 16]> {
        self.raw.opt_clsid()
    }

    /// Icon size, or `None` unless `FD_SIZEPOINT` is set.
    #[must_use]
    pub const fn icon_size(&self) -> Option<SizeL> {
        self.raw.opt_icon_size()
    }

    /// Icon position, or `None` unless `FD_SIZEPOINT` is set.
    #[must_use]
    pub const fn icon_position(&self) -> Option<PointL> {
        self.raw.opt_icon_position()
    }

    /// `dwFileAttributes`, or `None` unless `FD_ATTRIBUTES` is set.
    #[must_use]
    pub const fn file_attributes(&self) -> Option<u32> {
        self.raw.opt_file_attributes()
    }

    /// `ftCreationTime` in 100ns ticks since 1601-01-01 UTC, or `None` unless
    /// `FD_CREATETIME` is set.
    #[must_use]
    pub const fn creation_time(&self) -> Option<u64> {
        self.raw.opt_creation_time()
    }

    /// `ftLastAccessTime`, or `None` unless `FD_ACCESSTIME` is set.
    #[must_use]
    pub const fn last_access_time(&self) -> Option<u64> {
        self.raw.opt_last_access_time()
    }

    /// `ftLastWriteTime`, or `None` unless `FD_WRITESTIME` is set.
    #[must_use]
    pub const fn last_write_time(&self) -> Option<u64> {
        self.raw.opt_last_write_time()
    }

    /// File size in bytes, or `None` unless `FD_FILESIZE` is set.
    #[must_use]
    pub const fn file_size(&self) -> Option<u64> {
        self.raw.opt_file_size()
    }

    /// `FD_PROGRESSUI`: the source wants a progress indicator shown.
    #[must_use]
    pub const fn wants_progress_ui(&self) -> bool {
        self.raw.flags.contains(Flags::PROGRESSUI)
    }

    /// `FD_LINKUI`: treat the transfer as a shortcut.
    #[must_use]
    pub const fn is_shortcut(&self) -> bool {
        self.raw.flags.contains(Flags::LINKUI)
    }

    /// `FD_UNICODE` — which on an *ANSI* descriptor is a contradiction.
    ///
    /// The flag says "the file name is Unicode", and this struct's name field is
    /// 260 single bytes. Reported rather than rejected: a producer that sets it
    /// here is confused, and knowing that is more useful to a caller deciding
    /// how much to trust the payload than a refusal to parse would be. The name
    /// is still handed back as bytes either way.
    #[must_use]
    pub const fn claims_unicode(&self) -> bool {
        self.raw.flags.contains(Flags::UNICODE)
    }

    /// `true` if the attributes are stated *and* say this is a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.raw.is_directory()
    }
}

/// A parsed `FILEGROUPDESCRIPTORA`.
///
/// Borrows the input; parsing allocates nothing and never sizes anything from
/// `cItems`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileGroupDescriptorA<'a> {
    count: usize,
    items: &'a [u8],
}

impl<'a> FileGroupDescriptorA<'a> {
    /// Parse a `CFSTR_FILEDESCRIPTORA` payload.
    ///
    /// Trailing bytes beyond the last descriptor are ignored: the payload
    /// arrives in an `HGLOBAL`, and `GlobalAlloc` rounds capacity up.
    ///
    /// There is no sniffing between this and [`crate::FileGroupDescriptor`].
    /// Which one a payload is comes from the *format name* it was offered
    /// under — `"FileGroupDescriptor"` here, `"FileGroupDescriptorW"` there —
    /// and guessing from the length would be wrong exactly when the trailing
    /// slack made both readings fit.
    ///
    /// # Errors
    ///
    /// - [`ErrorKind::UnexpectedEof`] if the `cItems` word itself is missing.
    /// - [`ErrorKind::TooLarge`] if `cItems` claims more descriptors than the
    ///   buffer could possibly hold.
    pub fn parse(buf: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(buf);
        let c_items = r.u32_le()?;
        debug_assert_eq!(r.pos(), GROUP_HEADER_LEN);

        let count = usize::try_from(c_items).map_err(|_| Error::new(ErrorKind::TooLarge, 0))?;
        // `cItems` is a u32 straight off another process's clipboard. Check it
        // against what is actually here before multiplying by 332.
        r.check_count(count, DESCRIPTOR_A_LEN)?;
        // check_count just proved this product fits in the remaining input.
        let items = r.take(count * DESCRIPTOR_A_LEN)?;

        Ok(Self { count, items })
    }

    /// Number of descriptors, as validated against the buffer.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    /// `true` if the group declares no files. Legal, if pointless.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The descriptor at `index`, or `None` if out of range.
    ///
    /// `index` is also the `FORMATETC::lindex` to ask for this file's
    /// `CFSTR_FILECONTENTS` with.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<FileDescriptorA<'a>> {
        let start = index.checked_mul(DESCRIPTOR_A_LEN)?;
        let end = start.checked_add(DESCRIPTOR_A_LEN)?;
        let bytes = self.items.get(start..end)?;
        // Infallible: `bytes` is exactly DESCRIPTOR_A_LEN long by construction.
        FileDescriptorA::parse(bytes).ok()
    }

    /// Iterate the descriptors. Borrows; allocates nothing.
    #[must_use]
    pub const fn iter(&self) -> Descriptors<'a> {
        Descriptors { rest: self.items }
    }

    /// The descriptor array as raw bytes, `cItems` excluded.
    #[must_use]
    pub const fn raw_items(&self) -> &'a [u8] {
        self.items
    }
}

impl<'a> IntoIterator for &FileGroupDescriptorA<'a> {
    type Item = FileDescriptorA<'a>;
    type IntoIter = Descriptors<'a>;

    fn into_iter(self) -> Descriptors<'a> {
        self.iter()
    }
}

/// Iterator over the descriptors of a [`FileGroupDescriptorA`].
///
/// Infallible: parsing already proved every 332-byte slot is present.
#[derive(Debug, Clone)]
pub struct Descriptors<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Descriptors<'a> {
    type Item = FileDescriptorA<'a>;

    fn next(&mut self) -> Option<FileDescriptorA<'a>> {
        if self.rest.len() < DESCRIPTOR_A_LEN {
            return None;
        }
        let (head, tail) = self.rest.split_at(DESCRIPTOR_A_LEN);
        self.rest = tail;
        // Infallible: `head` is exactly DESCRIPTOR_A_LEN long, which is the only
        // way `FileDescriptorA::parse` can fail.
        FileDescriptorA::parse(head).ok()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.rest.len() / DESCRIPTOR_A_LEN;
        (n, Some(n))
    }
}

impl ExactSizeIterator for Descriptors<'_> {}

/// Builds a `FILEGROUPDESCRIPTORA` payload.
///
/// Names go in as **bytes**, already in whatever code page the receiver will
/// read them with. There is no `push(raw, &str)` here as there is for the wide
/// form, because there is no encoding to convert a `&str` *into* without being
/// told one — turn on the `codepage` feature and use
/// [`Builder::push_str_with`](#method.push_str_with) to name it.
///
/// Behind the `alloc` feature because it owns its output.
#[cfg(feature = "alloc")]
#[derive(Debug, Clone, Default)]
pub struct Builder {
    items: Vec<u8>,
    count: u32,
}

#[cfg(feature = "alloc")]
impl Builder {
    /// An empty group.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            items: Vec::new(),
            count: 0,
        }
    }

    /// Append a descriptor whose name is already encoded, without a NUL.
    ///
    /// The round-trip path: feed it [`FileDescriptorA::file_name_ansi`].
    ///
    /// # Errors
    ///
    /// [`ErrorKind::TooLarge`] if the name does not leave room for a terminator
    /// within [`FILE_NAME_BYTES`], and [`ErrorKind::Malformed`] for an embedded
    /// NUL, which would truncate the name on the way back in and silently make
    /// it a different file.
    pub fn push_ansi_name(&mut self, raw: RawDescriptor, name: &[u8]) -> Result<()> {
        let at = self.items.len();
        if name.len() > MAX_WRITABLE_NAME_BYTES {
            return Err(Error::new(ErrorKind::TooLarge, at));
        }
        if name.contains(&0) {
            return Err(Error::new(ErrorKind::Malformed, at));
        }
        self.write_fixed(raw);
        self.items.extend_from_slice(name);
        self.pad_name(name.len());
        self.count += 1;
        Ok(())
    }

    /// Append a descriptor the parser produced, byte-for-byte.
    ///
    /// # Errors
    ///
    /// As [`Self::push_ansi_name`].
    pub fn push_descriptor(&mut self, d: &FileDescriptorA<'_>) -> Result<()> {
        self.push_ansi_name(d.raw(), d.file_name_ansi())
    }

    /// Number of descriptors so far.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count as usize
    }

    /// `true` if nothing has been pushed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Emit the payload: `cItems` followed by the descriptor array.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(GROUP_HEADER_LEN + self.items.len());
        out.extend_from_slice(&self.count.to_le_bytes());
        out.extend_from_slice(&self.items);
        out
    }

    /// Write the 72 bytes that precede `cFileName`.
    fn write_fixed(&mut self, raw: RawDescriptor) {
        let start = self.items.len();
        self.items
            .extend_from_slice(&raw.flags.bits().to_le_bytes());
        self.items.extend_from_slice(&raw.clsid);
        self.items
            .extend_from_slice(&raw.icon_size.cx.to_le_bytes());
        self.items
            .extend_from_slice(&raw.icon_size.cy.to_le_bytes());
        self.items
            .extend_from_slice(&raw.icon_position.x.to_le_bytes());
        self.items
            .extend_from_slice(&raw.icon_position.y.to_le_bytes());
        self.items
            .extend_from_slice(&raw.file_attributes.to_le_bytes());
        crate::write_filetime(&mut self.items, raw.creation_time);
        crate::write_filetime(&mut self.items, raw.last_access_time);
        crate::write_filetime(&mut self.items, raw.last_write_time);
        // nFileSizeHigh comes first in the declaration, so the size cannot be
        // written as one little-endian u64 the way FILETIME can.
        let high = (raw.file_size >> 32) as u32;
        let low = raw.file_size as u32;
        self.items.extend_from_slice(&high.to_le_bytes());
        self.items.extend_from_slice(&low.to_le_bytes());
        debug_assert_eq!(self.items.len() - start, OFF_FILENAME);
    }

    /// NUL-terminate the name and zero-fill the rest of the 260-byte field, so
    /// every descriptor is exactly 332 bytes regardless of name length.
    fn pad_name(&mut self, bytes_written: usize) {
        let remaining = FILE_NAME_BYTES - bytes_written;
        self.items.resize(self.items.len() + remaining, 0);
    }
}

/// Decoding and encoding `cFileName` with a named legacy code page.
///
/// The system ANSI code page is a property of the machine that wrote the
/// payload and is not in the payload. Everything here therefore takes the
/// encoding as a parameter; nothing in this module guesses.
#[cfg(feature = "codepage")]
mod with_codepage {
    #[cfg(feature = "alloc")]
    extern crate alloc;

    use rclip_codepage::{Decoder, Encoding};
    #[cfg(feature = "alloc")]
    use rclip_core::{Error, ErrorKind, Result};

    use super::FileDescriptorA;
    #[cfg(feature = "alloc")]
    use super::MAX_WRITABLE_NAME_BYTES;

    impl<'a> FileDescriptorA<'a> {
        /// Decode the name a `char` at a time with a named code page.
        ///
        /// An undefined byte yields an error and iteration continues: a
        /// single-byte code page cannot lose sync, so the rest of the name is
        /// still worth reading.
        #[must_use]
        pub const fn chars_with(&self, enc: Encoding) -> Decoder<'a> {
            enc.decode(self.file_name_ansi())
        }

        /// Decode the name with a named code page, failing on anything the
        /// page leaves undefined.
        ///
        /// # Errors
        ///
        /// [`ErrorKind::Malformed`] at the first undefined byte. A name that
        /// does not round-trip is worth knowing about before you create a file
        /// with it, which is why this exists alongside the lossy form.
        #[cfg(feature = "alloc")]
        pub fn file_name_with(&self, enc: Encoding) -> Result<alloc::string::String> {
            let mut out = alloc::string::String::new();
            for c in self.chars_with(enc) {
                out.push(c?);
            }
            Ok(out)
        }

        /// Decode the name with a named code page, substituting U+FFFD.
        #[cfg(feature = "alloc")]
        #[must_use]
        pub fn file_name_lossy_with(&self, enc: Encoding) -> alloc::string::String {
            let mut out = alloc::string::String::new();
            for c in self.chars_with(enc) {
                out.push(c.unwrap_or('\u{FFFD}'));
            }
            out
        }
    }

    #[cfg(feature = "alloc")]
    impl super::Builder {
        /// Append a descriptor, encoding a Rust string with `enc`.
        ///
        /// The counterpart to [`super::Builder::push_ansi_name`], which takes
        /// bytes precisely because this crate has no code page of its own to
        /// encode with.
        ///
        /// # Errors
        ///
        /// [`ErrorKind::Unsupported`] if `enc` cannot represent a character in
        /// `name` — a substitution here would be a *different file name*, not a
        /// display glitch. [`ErrorKind::TooLarge`] if the encoded name does not
        /// leave room for a terminator, and [`ErrorKind::Malformed`] for an
        /// embedded NUL.
        pub fn push_str_with(
            &mut self,
            raw: super::RawDescriptor,
            name: &str,
            enc: Encoding,
        ) -> Result<()> {
            let at = self.len();
            let mut bytes = alloc::vec::Vec::with_capacity(name.len());
            for c in name.chars() {
                let b = enc
                    .encode_char(c)
                    .ok_or(Error::new(ErrorKind::Unsupported, at))?;
                bytes.push(b);
            }
            // Measured after encoding, not before: in a double-byte page one
            // character can cost two of the 260 bytes, and `MAX_PATH` is a byte
            // count here rather than a character count.
            if bytes.len() > MAX_WRITABLE_NAME_BYTES {
                return Err(Error::new(ErrorKind::TooLarge, at));
            }
            self.push_ansi_name(raw, &bytes)
        }
    }
}

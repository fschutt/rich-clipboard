//! `CFSTR_FILEDESCRIPTORW` — descriptors for files that do not exist on disk.
//!
//! This is how Outlook drags an attachment that lives in a database, and how an
//! application can offer "drag this generated PDF into Explorer" without first
//! writing a temp file. The descriptor says what the file *would* be called and
//! how big it *would* be; the bytes are fetched separately, one
//! `CFSTR_FILECONTENTS` per descriptor, keyed by the descriptor's zero-based
//! index in `FORMATETC::lindex`. Only the descriptor is a plain struct, so only
//! the descriptor is in scope here — the contents are transport.
//!
//! TODO(phase-1): `CFSTR_FILECONTENTS` itself. It is an `IStream` (or an
//! `HGLOBAL`, or an `IStorage`) rather than a byte layout, so it belongs to the
//! platform backend and not to a codec.
//!
//! TODO(phase-1): `FILEGROUPDESCRIPTORA`. Same shape with `CHAR cFileName[260]`,
//! so 332 bytes per descriptor, and the same unknowable-codepage problem as
//! `CF_HDROP`'s `fWide == 0`. Deferred until a real capture needs it.
//!
//! ```text
//! FILEGROUPDESCRIPTORW
//!   offset 0   cItems : UINT              number of descriptors
//!          4   fgd[]  : FILEDESCRIPTORW   cItems × 592 bytes
//!
//! FILEDESCRIPTORW                                             (592 bytes)
//!   offset   0   dwFlags          : DWORD      which fields below mean anything
//!            4   clsid            : CLSID      16 bytes
//!           20   sizel            : SIZEL      LONG cx, LONG cy — icon size
//!           28   pointl           : POINTL     LONG x,  LONG y  — icon position
//!           36   dwFileAttributes : DWORD      FILE_ATTRIBUTE_*
//!           40   ftCreationTime   : FILETIME   two DWORDs, 100ns ticks since 1601
//!           48   ftLastAccessTime : FILETIME
//!           56   ftLastWriteTime  : FILETIME
//!           64   nFileSizeHigh    : DWORD
//!           68   nFileSizeLow     : DWORD
//!           72   cFileName[260]   : WCHAR      520 bytes, NUL-truncated
//! ```
//!
//! There is **no padding anywhere in that table**, and that is worth stating
//! because it looks like there should be: `FILETIME` is 64 bits of data but is
//! declared as two `DWORD`s, so its alignment is 4, not 8. Every member of
//! `FILEDESCRIPTORW` is 4-byte aligned, so the struct's alignment is 4, its
//! size is exactly 592, and `fgd` starts at offset 4 rather than 8. A reader
//! that assumes natural 8-byte alignment for the timestamps is off by four
//! bytes for the whole array.
//!
//! # Parsing
//!
//! ```
//! use rclip_file_desc::FileGroupDescriptor;
//!
//! # fn main() -> Result<(), rclip_core::Error> {
//! # let bytes = &make();
//! # fn make() -> Vec<u8> {
//! #     let mut v = vec![1u8, 0, 0, 0];
//! #     v.extend_from_slice(&0x40u32.to_le_bytes());
//! #     v.resize(4 + 64, 0);
//! #     v.extend_from_slice(&0u32.to_le_bytes());
//! #     v.extend_from_slice(&7u32.to_le_bytes());
//! #     for u in "note.txt".encode_utf16() { v.extend_from_slice(&u.to_le_bytes()); }
//! #     v.resize(4 + 592, 0);
//! #     v
//! # }
//! let group = FileGroupDescriptor::parse(bytes)?;
//! for d in group.iter() {
//!     assert_eq!(d.file_size(), Some(7));
//!     // A flag that is clear reads as `None`, never as a plausible zero.
//!     assert_eq!(d.last_write_time(), None);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! [FILEDESCRIPTORW]: https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-filedescriptorw
//! [FILEGROUPDESCRIPTORW]: https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-filegroupdescriptorw

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use core::fmt;

use rclip_core::{Error, ErrorKind, Reader, Result, Utf16Le};

/// Size of one `FILEDESCRIPTORW`, in bytes.
///
/// 4 + 16 + 8 + 8 + 4 + 8 + 8 + 8 + 4 + 4 + 520. See the module docs for why
/// there is no padding to add to that.
pub const DESCRIPTOR_LEN: usize = 592;

/// Size of the `cItems` field that precedes the descriptor array.
pub const GROUP_HEADER_LEN: usize = 4;

/// Capacity of `cFileName`, in UTF-16 code units. This is Win32 `MAX_PATH`.
pub const FILE_NAME_UNITS: usize = 260;

/// Longest name this crate will *write*: [`FILE_NAME_UNITS`] less the NUL.
///
/// The field is documented as holding "the null-terminated string that contains
/// the name of the file", so a name that fills all 260 units leaves nowhere for
/// the terminator and makes every reader run off the end of the field.
pub const MAX_WRITABLE_NAME_UNITS: usize = FILE_NAME_UNITS - 1;

// Field offsets within one descriptor, for the serializer and for tests.
const OFF_FLAGS: usize = 0;
const OFF_CLSID: usize = 4;
const OFF_SIZEL: usize = 20;
const OFF_POINTL: usize = 28;
const OFF_ATTRIBUTES: usize = 36;
const OFF_CREATION: usize = 40;
const OFF_ACCESS: usize = 48;
const OFF_WRITE: usize = 56;
const OFF_SIZE_HIGH: usize = 64;
const OFF_SIZE_LOW: usize = 68;
const OFF_FILENAME: usize = 72;

const _: () = assert!(OFF_FILENAME + FILE_NAME_UNITS * 2 == DESCRIPTOR_LEN);

/// `dwFlags` — which of the other members contain valid data.
///
/// A hand-rolled newtype rather than the `bitflags` crate: ten constants and
/// three operators do not justify a dependency in a `no_std` codec.
///
/// The reason this type matters is that a clear flag and a zero field look
/// identical on the wire. `dwFileAttributes == 0` with `FD_ATTRIBUTES` clear
/// means "not stated"; with it set it means "no attributes". The accessors on
/// [`FileDescriptor`] return `Option` so those two cannot be confused.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Flags(u32);

impl Flags {
    /// No flags set.
    pub const NONE: Self = Self(0);
    /// `FD_CLSID` — `clsid` is valid.
    pub const CLSID: Self = Self(0x0000_0001);
    /// `FD_SIZEPOINT` — `sizel` and `pointl` are valid.
    pub const SIZEPOINT: Self = Self(0x0000_0002);
    /// `FD_ATTRIBUTES` — `dwFileAttributes` is valid.
    pub const ATTRIBUTES: Self = Self(0x0000_0004);
    /// `FD_CREATETIME` — `ftCreationTime` is valid.
    pub const CREATETIME: Self = Self(0x0000_0008);
    /// `FD_ACCESSTIME` — `ftLastAccessTime` is valid.
    pub const ACCESSTIME: Self = Self(0x0000_0010);
    /// `FD_WRITESTIME` — `ftLastWriteTime` is valid.
    pub const WRITESTIME: Self = Self(0x0000_0020);
    /// `FD_FILESIZE` — `nFileSizeHigh` and `nFileSizeLow` are valid.
    ///
    /// Set this with both halves zero to promise a zero-length file; leave it
    /// clear and the size is simply unknown until the contents arrive.
    pub const FILESIZE: Self = Self(0x0000_0040);
    /// `FD_PROGRESSUI` — show a progress indicator during the transfer.
    pub const PROGRESSUI: Self = Self(0x0000_4000);
    /// `FD_LINKUI` — treat the operation as a shortcut.
    ///
    /// Legacy. Microsoft's own guidance: "Before Microsoft Internet Explorer
    /// 4.0, an application indicated that it was transferring shortcut file
    /// types by setting FD_LINKUI […]. Now, the preferred way to indicate that
    /// shortcuts are being transferred is to use the `CFSTR_PREFERREDDROPEFFECT`
    /// format set to `DROPEFFECT_LINK`. However, for backward compatibility
    /// with older systems, sources should still set the FD_LINKUI flag."
    /// So: set both, and read `CFSTR_PREFERREDDROPEFFECT` in preference to this
    /// bit. That format is a bare `DWORD` and is not this crate's business —
    /// see `rclip_core::flavor::drop_effect`.
    pub const LINKUI: Self = Self(0x0000_8000);
    /// `FD_UNICODE` — the descriptor is Unicode. Windows Vista and later.
    ///
    /// Redundant in `CFSTR_FILEDESCRIPTORW`, whose `W` already says so; it
    /// exists because the ANSI and Unicode structures are otherwise
    /// indistinguishable once they are loose bytes in an `HGLOBAL`.
    pub const UNICODE: Self = Self(0x8000_0000);

    /// Every bit this crate knows a name for.
    pub const KNOWN: Self = Self(
        Self::CLSID.0
            | Self::SIZEPOINT.0
            | Self::ATTRIBUTES.0
            | Self::CREATETIME.0
            | Self::ACCESSTIME.0
            | Self::WRITESTIME.0
            | Self::FILESIZE.0
            | Self::PROGRESSUI.0
            | Self::LINKUI.0
            | Self::UNICODE.0,
    );

    /// Wrap a raw `dwFlags` word, unknown bits and all.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The raw word, unknown bits preserved.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// `true` if every bit of `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// `true` if any bit of `other` is set.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// This word with `other` set.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// This word with `other` cleared.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Bits set that this crate has no name for. Not an error — `dwFlags` is a
    /// `DWORD` and Microsoft has added bits before.
    #[must_use]
    pub const fn unknown_bits(self) -> u32 {
        self.0 & !Self::KNOWN.0
    }
}

impl core::ops::BitOr for Flags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.with(rhs)
    }
}

impl core::ops::BitOrAssign for Flags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAnd for Flags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl fmt::Debug for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [(Flags, &str); 10] = [
            (Flags::CLSID, "CLSID"),
            (Flags::SIZEPOINT, "SIZEPOINT"),
            (Flags::ATTRIBUTES, "ATTRIBUTES"),
            (Flags::CREATETIME, "CREATETIME"),
            (Flags::ACCESSTIME, "ACCESSTIME"),
            (Flags::WRITESTIME, "WRITESTIME"),
            (Flags::FILESIZE, "FILESIZE"),
            (Flags::PROGRESSUI, "PROGRESSUI"),
            (Flags::LINKUI, "LINKUI"),
            (Flags::UNICODE, "UNICODE"),
        ];
        f.write_str("Flags(")?;
        let mut first = true;
        for (flag, name) in NAMES {
            if self.contains(flag) {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        let unknown = self.unknown_bits();
        if unknown != 0 {
            if !first {
                f.write_str("|")?;
            }
            write!(f, "{unknown:#010x}")?;
            first = false;
        }
        if first {
            f.write_str("NONE")?;
        }
        f.write_str(")")
    }
}

/// A Win32 `SIZEL`: two `LONG`s.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct SizeL {
    /// Width.
    pub cx: i32,
    /// Height.
    pub cy: i32,
}

/// A Win32 `POINTL`: two `LONG`s.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct PointL {
    /// Horizontal coordinate.
    pub x: i32,
    /// Vertical coordinate.
    pub y: i32,
}

/// A few `FILE_ATTRIBUTE_*` values, for reading `dwFileAttributes`.
///
/// Not exhaustive — the full list belongs to `GetFileAttributes`, and the value
/// is passed through verbatim rather than validated.
pub mod file_attribute {
    /// `FILE_ATTRIBUTE_READONLY`.
    pub const READONLY: u32 = 0x0000_0001;
    /// `FILE_ATTRIBUTE_HIDDEN`.
    pub const HIDDEN: u32 = 0x0000_0002;
    /// `FILE_ATTRIBUTE_SYSTEM`.
    pub const SYSTEM: u32 = 0x0000_0004;
    /// `FILE_ATTRIBUTE_DIRECTORY`. A descriptor with this set names a folder
    /// and has no `CFSTR_FILECONTENTS` of its own.
    pub const DIRECTORY: u32 = 0x0000_0010;
    /// `FILE_ATTRIBUTE_ARCHIVE`.
    pub const ARCHIVE: u32 = 0x0000_0020;
    /// `FILE_ATTRIBUTE_NORMAL`. Valid only on its own.
    pub const NORMAL: u32 = 0x0000_0080;
}

/// Every fixed-width field of a descriptor, exactly as it sat on the wire.
///
/// Flags are *not* applied here: a field whose flag is clear keeps whatever
/// bytes the producer left in it. Use this to re-serialize a descriptor
/// byte-for-byte, and [`FileDescriptor`]'s accessors to read one meaningfully.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct RawDescriptor {
    /// `dwFlags`.
    pub flags: Flags,
    /// `clsid`, in packet representation (MS-DTYP 2.3.4.2).
    pub clsid: [u8; 16],
    /// `sizel` — the icon's size.
    pub icon_size: SizeL,
    /// `pointl` — the icon's position.
    pub icon_position: PointL,
    /// `dwFileAttributes`.
    pub file_attributes: u32,
    /// `ftCreationTime` as one `u64` of 100ns ticks since 1601-01-01 UTC.
    pub creation_time: u64,
    /// `ftLastAccessTime`, same units.
    pub last_access_time: u64,
    /// `ftLastWriteTime`, same units.
    pub last_write_time: u64,
    /// `nFileSizeHigh` and `nFileSizeLow` recombined.
    pub file_size: u64,
}

impl RawDescriptor {
    /// An all-zero descriptor with no flags set: nothing is claimed about the
    /// file except its name.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            flags: Flags::NONE,
            clsid: [0; 16],
            icon_size: SizeL { cx: 0, cy: 0 },
            icon_position: PointL { x: 0, y: 0 },
            file_attributes: 0,
            creation_time: 0,
            last_access_time: 0,
            last_write_time: 0,
            file_size: 0,
        }
    }

    /// Declare the file size, setting `FD_FILESIZE`.
    ///
    /// Use this rather than assigning the field: writing a size without the
    /// flag is the standard way to produce a descriptor that Explorer shows as
    /// zero bytes, and it is invisible in a hex dump unless you are looking for
    /// it.
    #[must_use]
    pub const fn with_file_size(mut self, bytes: u64) -> Self {
        self.file_size = bytes;
        self.flags = self.flags.with(Flags::FILESIZE);
        self
    }

    /// Declare `dwFileAttributes`, setting `FD_ATTRIBUTES`.
    #[must_use]
    pub const fn with_attributes(mut self, attrs: u32) -> Self {
        self.file_attributes = attrs;
        self.flags = self.flags.with(Flags::ATTRIBUTES);
        self
    }

    /// Declare `ftCreationTime`, setting `FD_CREATETIME`.
    #[must_use]
    pub const fn with_creation_time(mut self, filetime: u64) -> Self {
        self.creation_time = filetime;
        self.flags = self.flags.with(Flags::CREATETIME);
        self
    }

    /// Declare `ftLastAccessTime`, setting `FD_ACCESSTIME`.
    #[must_use]
    pub const fn with_last_access_time(mut self, filetime: u64) -> Self {
        self.last_access_time = filetime;
        self.flags = self.flags.with(Flags::ACCESSTIME);
        self
    }

    /// Declare `ftLastWriteTime`, setting `FD_WRITESTIME`.
    #[must_use]
    pub const fn with_last_write_time(mut self, filetime: u64) -> Self {
        self.last_write_time = filetime;
        self.flags = self.flags.with(Flags::WRITESTIME);
        self
    }

    /// Declare the file type identifier, setting `FD_CLSID`.
    #[must_use]
    pub const fn with_clsid(mut self, clsid: [u8; 16]) -> Self {
        self.clsid = clsid;
        self.flags = self.flags.with(Flags::CLSID);
        self
    }

    /// Declare the icon size and position, setting `FD_SIZEPOINT`.
    ///
    /// One flag covers both members, so they are set together.
    #[must_use]
    pub const fn with_icon(mut self, size: SizeL, position: PointL) -> Self {
        self.icon_size = size;
        self.icon_position = position;
        self.flags = self.flags.with(Flags::SIZEPOINT);
        self
    }

    /// Ask the shell for a progress indicator (`FD_PROGRESSUI`).
    #[must_use]
    pub const fn with_progress_ui(mut self) -> Self {
        self.flags = self.flags.with(Flags::PROGRESSUI);
        self
    }

    /// Mark the transfer as a shortcut (`FD_LINKUI`).
    ///
    /// Also publish `CFSTR_PREFERREDDROPEFFECT` = `DROPEFFECT_LINK`; see
    /// [`Flags::LINKUI`].
    #[must_use]
    pub const fn with_shortcut(mut self) -> Self {
        self.flags = self.flags.with(Flags::LINKUI);
        self
    }

    /// Set `FD_UNICODE`.
    #[must_use]
    pub const fn with_unicode(mut self) -> Self {
        self.flags = self.flags.with(Flags::UNICODE);
        self
    }
}

/// One `FILEDESCRIPTORW`.
///
/// Borrows the name from the input buffer; constructing one allocates nothing.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct FileDescriptor<'a> {
    raw: RawDescriptor,
    /// `cFileName` truncated at its first NUL, still UTF-16LE.
    name: &'a [u8],
}

impl<'a> FileDescriptor<'a> {
    /// Parse one descriptor from exactly [`DESCRIPTOR_LEN`] bytes.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::BadLength`] if `bytes` is not exactly 592 long. Every field
    /// is fixed-width, so nothing else here can fail.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() != DESCRIPTOR_LEN {
            return Err(Error::new(ErrorKind::BadLength, 0));
        }
        let mut r = Reader::new(bytes);
        let flags = Flags::from_bits(r.u32_le()?);
        let clsid = r.guid()?;
        let icon_size = SizeL {
            cx: r.i32_le()?,
            cy: r.i32_le()?,
        };
        let icon_position = PointL {
            x: r.i32_le()?,
            y: r.i32_le()?,
        };
        let file_attributes = r.u32_le()?;
        let creation_time = filetime(&mut r)?;
        let last_access_time = filetime(&mut r)?;
        let last_write_time = filetime(&mut r)?;
        // The size arrives as two DWORDs, high first in the declaration but
        // little-endian on the wire, so read both and recombine rather than
        // reading a u64 across them.
        let high = r.u32_le()?;
        let low = r.u32_le()?;
        let file_size = (u64::from(high) << 32) | u64::from(low);
        debug_assert_eq!(r.pos(), OFF_FILENAME);
        // Fixed 260-unit field: the name is whatever precedes the first NUL,
        // and the rest of the field is padding that must not leak into it.
        let name = r.utf16_fixed(FILE_NAME_UNITS)?;

        let raw = RawDescriptor {
            flags,
            clsid,
            icon_size,
            icon_position,
            file_attributes,
            creation_time,
            last_access_time,
            last_write_time,
            file_size,
        };
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

    /// `cFileName` as UTF-16LE bytes, NUL and trailing padding removed.
    ///
    /// Despite the name, real producers put a *relative path* in here when they
    /// describe a folder tree — `sub\file.txt`, backslash-separated. This crate
    /// returns it as it arrived and resolves nothing.
    #[must_use]
    pub const fn file_name_utf16(&self) -> &'a [u8] {
        self.name
    }

    /// Decode the name a `char` at a time, reporting lone surrogates.
    #[must_use]
    pub const fn file_name_chars(&self) -> Utf16Le<'a> {
        Utf16Le::new(self.name)
    }

    /// Decode the name, substituting U+FFFD for malformed sequences.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn file_name_lossy(&self) -> alloc::string::String {
        rclip_core::utf16::decode_utf16le_lossy(self.name)
    }

    /// `clsid`, or `None` unless `FD_CLSID` is set.
    #[must_use]
    pub const fn clsid(&self) -> Option<[u8; 16]> {
        if self.raw.flags.contains(Flags::CLSID) {
            Some(self.raw.clsid)
        } else {
            None
        }
    }

    /// Icon size, or `None` unless `FD_SIZEPOINT` is set.
    #[must_use]
    pub const fn icon_size(&self) -> Option<SizeL> {
        if self.raw.flags.contains(Flags::SIZEPOINT) {
            Some(self.raw.icon_size)
        } else {
            None
        }
    }

    /// Icon position, or `None` unless `FD_SIZEPOINT` is set.
    #[must_use]
    pub const fn icon_position(&self) -> Option<PointL> {
        if self.raw.flags.contains(Flags::SIZEPOINT) {
            Some(self.raw.icon_position)
        } else {
            None
        }
    }

    /// `dwFileAttributes`, or `None` unless `FD_ATTRIBUTES` is set.
    #[must_use]
    pub const fn file_attributes(&self) -> Option<u32> {
        if self.raw.flags.contains(Flags::ATTRIBUTES) {
            Some(self.raw.file_attributes)
        } else {
            None
        }
    }

    /// `ftCreationTime` in 100ns ticks since 1601-01-01 UTC, or `None` unless
    /// `FD_CREATETIME` is set.
    ///
    /// Deliberately a bare `u64`: converting to a civil date needs a calendar,
    /// and a calendar is not a dependency a clipboard codec should carry.
    #[must_use]
    pub const fn creation_time(&self) -> Option<u64> {
        if self.raw.flags.contains(Flags::CREATETIME) {
            Some(self.raw.creation_time)
        } else {
            None
        }
    }

    /// `ftLastAccessTime`, or `None` unless `FD_ACCESSTIME` is set.
    #[must_use]
    pub const fn last_access_time(&self) -> Option<u64> {
        if self.raw.flags.contains(Flags::ACCESSTIME) {
            Some(self.raw.last_access_time)
        } else {
            None
        }
    }

    /// `ftLastWriteTime`, or `None` unless `FD_WRITESTIME` is set.
    #[must_use]
    pub const fn last_write_time(&self) -> Option<u64> {
        if self.raw.flags.contains(Flags::WRITESTIME) {
            Some(self.raw.last_write_time)
        } else {
            None
        }
    }

    /// File size in bytes, or `None` unless `FD_FILESIZE` is set.
    ///
    /// `Some(0)` is a real answer — the documented way to promise a zero-length
    /// file is `FD_FILESIZE` with both halves zero — which is precisely why
    /// this returns an `Option` rather than a `u64`.
    #[must_use]
    pub const fn file_size(&self) -> Option<u64> {
        if self.raw.flags.contains(Flags::FILESIZE) {
            Some(self.raw.file_size)
        } else {
            None
        }
    }

    /// `FD_PROGRESSUI`: the source wants a progress indicator shown.
    #[must_use]
    pub const fn wants_progress_ui(&self) -> bool {
        self.raw.flags.contains(Flags::PROGRESSUI)
    }

    /// `FD_LINKUI`: treat the transfer as a shortcut. Legacy; prefer
    /// `CFSTR_PREFERREDDROPEFFECT` = `DROPEFFECT_LINK`. See [`Flags::LINKUI`].
    #[must_use]
    pub const fn is_shortcut(&self) -> bool {
        self.raw.flags.contains(Flags::LINKUI)
    }

    /// `FD_UNICODE`.
    #[must_use]
    pub const fn is_unicode(&self) -> bool {
        self.raw.flags.contains(Flags::UNICODE)
    }

    /// `true` if the attributes are stated *and* say this is a directory.
    ///
    /// A directory descriptor has no `CFSTR_FILECONTENTS` of its own; it exists
    /// so the target knows to create the folder the following descriptors'
    /// relative names refer to.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        match self.file_attributes() {
            Some(a) => a & file_attribute::DIRECTORY != 0,
            None => false,
        }
    }
}

fn filetime(r: &mut Reader<'_>) -> Result<u64> {
    // FILETIME is { DWORD dwLowDateTime; DWORD dwHighDateTime; } — low first.
    // Reading it as a little-endian u64 gives the same answer, but spelling out
    // the two halves is what makes it obvious that this is not an 8-byte
    // aligned field.
    let low = r.u32_le()?;
    let high = r.u32_le()?;
    Ok((u64::from(high) << 32) | u64::from(low))
}

/// A parsed `FILEGROUPDESCRIPTORW`.
///
/// Borrows the input; parsing allocates nothing and never sizes anything from
/// `cItems`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileGroupDescriptor<'a> {
    count: usize,
    items: &'a [u8],
}

impl<'a> FileGroupDescriptor<'a> {
    /// Parse a `CFSTR_FILEDESCRIPTORW` payload.
    ///
    /// Trailing bytes beyond the last descriptor are ignored: the payload
    /// arrives in an `HGLOBAL`, and `GlobalAlloc` rounds capacity up.
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
        // against what is actually here before multiplying by 592 or handing it
        // to anything that iterates — 0xFFFFFFFF descriptors is a 2.5 TiB read
        // otherwise. This must happen before, not after, the take() below.
        r.check_count(count, DESCRIPTOR_LEN)?;
        // check_count just proved this product fits in the remaining input.
        let items = r.take(count * DESCRIPTOR_LEN)?;

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
    pub fn get(&self, index: usize) -> Option<FileDescriptor<'a>> {
        let start = index.checked_mul(DESCRIPTOR_LEN)?;
        let end = start.checked_add(DESCRIPTOR_LEN)?;
        let bytes = self.items.get(start..end)?;
        // Infallible: `bytes` is exactly DESCRIPTOR_LEN long by construction.
        FileDescriptor::parse(bytes).ok()
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

impl<'a> IntoIterator for &FileGroupDescriptor<'a> {
    type Item = FileDescriptor<'a>;
    type IntoIter = Descriptors<'a>;

    fn into_iter(self) -> Descriptors<'a> {
        self.iter()
    }
}

/// Iterator over the descriptors of a [`FileGroupDescriptor`].
///
/// Infallible: parsing already proved every 592-byte slot is present.
#[derive(Debug, Clone)]
pub struct Descriptors<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Descriptors<'a> {
    type Item = FileDescriptor<'a>;

    fn next(&mut self) -> Option<FileDescriptor<'a>> {
        if self.rest.len() < DESCRIPTOR_LEN {
            return None;
        }
        let (head, tail) = self.rest.split_at(DESCRIPTOR_LEN);
        self.rest = tail;
        // Infallible: `head` is exactly DESCRIPTOR_LEN long, which is the only
        // way `FileDescriptor::parse` can fail.
        FileDescriptor::parse(head).ok()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.rest.len() / DESCRIPTOR_LEN;
        (n, Some(n))
    }
}

impl ExactSizeIterator for Descriptors<'_> {}

/// Builds a `FILEGROUPDESCRIPTORW` payload.
///
/// The serializer is the point of this crate as much as the parser: it is what
/// lets an application offer a file it has not written to disk. Behind the
/// `alloc` feature because it owns its output.
///
/// ```
/// use rclip_file_desc::{Builder, FileGroupDescriptor, RawDescriptor, file_attribute};
///
/// # fn main() -> Result<(), rclip_core::Error> {
/// let mut b = Builder::new();
/// b.push(
///     RawDescriptor::new()
///         .with_file_size(4096)
///         .with_attributes(file_attribute::NORMAL)
///         .with_progress_ui(),
///     "report.pdf",
/// )?;
/// let bytes = b.finish();
///
/// let group = FileGroupDescriptor::parse(&bytes)?;
/// assert_eq!(group.len(), 1);
/// assert_eq!(group.get(0).unwrap().file_size(), Some(4096));
/// # Ok(())
/// # }
/// ```
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

    /// Append a descriptor with a Rust string for a name.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::TooLarge`] if the name needs more than
    /// [`MAX_WRITABLE_NAME_UNITS`] UTF-16 code units — note *units*, so an
    /// emoji costs two. [`ErrorKind::Malformed`] if the name contains a NUL,
    /// which would truncate it on the way back in.
    pub fn push(&mut self, raw: RawDescriptor, name: &str) -> Result<()> {
        if name.contains('\0') {
            return Err(Error::new(ErrorKind::Malformed, self.items.len()));
        }
        let units = name.encode_utf16().count();
        if units > MAX_WRITABLE_NAME_UNITS {
            return Err(Error::new(ErrorKind::TooLarge, self.items.len()));
        }
        self.write_fixed(raw);
        for unit in name.encode_utf16() {
            self.items.extend_from_slice(&unit.to_le_bytes());
        }
        self.pad_name(units);
        self.count += 1;
        Ok(())
    }

    /// Append a descriptor whose name is already UTF-16LE, without a NUL.
    ///
    /// The round-trip path: feed it [`FileDescriptor::file_name_utf16`].
    ///
    /// # Errors
    ///
    /// [`ErrorKind::BadLength`] for an odd byte count, [`ErrorKind::TooLarge`]
    /// if the name does not leave room for a terminator, and
    /// [`ErrorKind::Malformed`] for an embedded NUL unit.
    pub fn push_utf16_name(&mut self, raw: RawDescriptor, name: &[u8]) -> Result<()> {
        let at = self.items.len();
        if name.len() % 2 != 0 {
            return Err(Error::new(ErrorKind::BadLength, at));
        }
        let units = name.len() / 2;
        if units > MAX_WRITABLE_NAME_UNITS {
            return Err(Error::new(ErrorKind::TooLarge, at));
        }
        if name.chunks_exact(2).any(|u| u == [0, 0]) {
            return Err(Error::new(ErrorKind::Malformed, at));
        }
        self.write_fixed(raw);
        self.items.extend_from_slice(name);
        self.pad_name(units);
        self.count += 1;
        Ok(())
    }

    /// Append a descriptor the parser produced, byte-for-byte.
    ///
    /// # Errors
    ///
    /// As [`Self::push_utf16_name`].
    pub fn push_descriptor(&mut self, d: &FileDescriptor<'_>) -> Result<()> {
        self.push_utf16_name(d.raw(), d.file_name_utf16())
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
        write_filetime(&mut self.items, raw.creation_time);
        write_filetime(&mut self.items, raw.last_access_time);
        write_filetime(&mut self.items, raw.last_write_time);
        // nFileSizeHigh comes first in the declaration, so the size cannot be
        // written as one little-endian u64 the way FILETIME can. Both casts are
        // exact: `>> 32` and the low word each fit a u32 by construction.
        let high = (raw.file_size >> 32) as u32;
        let low = raw.file_size as u32;
        self.items.extend_from_slice(&high.to_le_bytes());
        self.items.extend_from_slice(&low.to_le_bytes());
        debug_assert_eq!(self.items.len() - start, OFF_FILENAME);
    }

    /// NUL-terminate the name and zero-fill the rest of the 260-unit field, so
    /// every descriptor is exactly 592 bytes regardless of name length.
    fn pad_name(&mut self, units_written: usize) {
        let remaining_units = FILE_NAME_UNITS - units_written;
        self.items.resize(self.items.len() + remaining_units * 2, 0);
    }
}

#[cfg(feature = "alloc")]
fn write_filetime(out: &mut Vec<u8>, ticks: u64) {
    // FILETIME is { DWORD dwLowDateTime; DWORD dwHighDateTime; }. Low DWORD
    // first, each little-endian, is byte-for-byte a little-endian u64 — unlike
    // nFileSizeHigh/nFileSizeLow, which are declared the other way round.
    out.extend_from_slice(&ticks.to_le_bytes());
}

// Keep the offset table honest: these constants document the layout in the
// module docs and are used by tests, so they must not drift.
const _: () = {
    assert!(OFF_FLAGS == 0);
    assert!(OFF_CLSID == 4);
    assert!(OFF_SIZEL == 20);
    assert!(OFF_POINTL == 28);
    assert!(OFF_ATTRIBUTES == 36);
    assert!(OFF_CREATION == 40);
    assert!(OFF_ACCESS == 48);
    assert!(OFF_WRITE == 56);
    assert!(OFF_SIZE_HIGH == 64);
    assert!(OFF_SIZE_LOW == 68);
};

//! [MS-PROPSTORE] serialized property storage — the payload of a
//! `PropertyStoreDataBlock`.
//!
//! [MS-PROPSTORE]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-propstore/39ea873f-7af5-44dd-92f9-bc1f293852cc
//!
//! ```text
//! PropertyStore   = *SerializedPropertyStorage u32(0)
//! Storage         { u32 StorageSize; u32 Version; GUID FormatID; *Value u32(0) }
//! Value(int name) { u32 ValueSize; u32 Id;       u8 Reserved; TypedPropertyValue }
//! Value(str name) { u32 ValueSize; u32 NameSize; u8 Reserved; Name; TypedPropertyValue }
//! ```
//!
//! # Why anyone cares
//!
//! This is where `System.AppUserModel.ID` lives — format ID
//! [`FMTID_APP_USER_MODEL`], property [`PID_APP_USER_MODEL_ID`]. That string is
//! how Windows decides which taskbar button a window belongs to and which
//! application a shortcut *is*: two shortcuts with the same AppUserModelID
//! group together, a pinned shortcut relaunches through it, and a Jump List
//! hangs off it. It is also the only place a `.lnk` states an application
//! identity that is not a path. [`PropertyStore::app_user_model_id`] is the
//! one-line accessor.
//!
//! # Nesting
//!
//! Two levels, both fixed: a store holds storages, a storage holds values, and
//! a value is a leaf. Nothing here recurses, so there is no depth to bound —
//! `VT_VECTOR | VT_VARIANT` would be the recursive case and it is deliberately
//! [`PropertyValue::Unsupported`].
//!
//! # Every length here came off the wire
//!
//! `StorageSize`, `ValueSize`, `NameSize` and each string's own length are all
//! attacker-chosen. Each level is read through a sub-[`Reader`] cut to its own
//! declared size, so an inner field that lies cannot reach past the record it
//! is in, let alone past the buffer. Sizes below the structural minimum are
//! rejected rather than skipped, because a size smaller than the header it
//! prefixes is also a walk that does not advance.
//!
//! # An unknown type must not cost the store
//!
//! Values are length-delimited by `ValueSize`, which is what makes it possible
//! to skip one without understanding it. A `VT_*` this crate does not decode —
//! or a payload too short for the type it claims — becomes
//! [`PropertyValue::Unsupported`] carrying its raw bytes, and the walk
//! continues to the next value. Only a broken *frame* (a size field that cannot
//! be true) ends it.

use rclip_core::{Error, ErrorKind, Reader, Result};
use rclip_idlist::{Guid, ShellStr};

use crate::filetime::FileTime;

/// `Version` in every serialized property storage: `0x53505331`, which is
/// `"1SPS"` on the wire.
pub const VERSION: u32 = 0x5350_5331;

/// Smallest legal `StorageSize`: the size field, `Version`, and `FormatID`.
///
/// A storage this small has no values at all, not even a terminator. That is
/// tolerated on the last storage in a buffer for the same reason a missing
/// terminator is; see [`PropertyStorages`].
pub const MIN_STORAGE_SIZE: u32 = 24;

/// Smallest legal `ValueSize`: `ValueSize`, `Id`/`NameSize`, `Reserved`, and a
/// four-byte `TypedPropertyValue` header. Matches libfwps.
pub const MIN_VALUE_SIZE: u32 = 13;

/// `TypedPropertyValue` header: the `Type` word plus its two padding bytes.
pub const TYPED_VALUE_HEADER: u32 = 4;

/// The format ID whose values are named by string rather than by integer:
/// `{D5CDD505-2E9C-101B-9397-08002B2CF9AE}`.
///
/// MS-PROPSTORE 2.2 makes this a property of the *storage*, not of the value:
/// every value in a storage with this format ID is a
/// [`Serialized Property Value (String Name)`][string-name], and every value in
/// any other storage is the integer-named form. There is no per-value
/// discriminator, so reading a value out of context is not possible.
///
/// [string-name]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-propstore/4720a485-d2e5-4a14-8f7e-4befd6516f9d
pub const FMTID_STRING_NAMED: Guid = Guid::from_bytes([
    0x05, 0xD5, 0xCD, 0xD5, 0x9C, 0x2E, 0x1B, 0x10, 0x93, 0x97, 0x08, 0x00, 0x2B, 0x2C, 0xF9, 0xAE,
]);

/// `{9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}` — the format ID of the
/// `System.AppUserModel.*` properties.
pub const FMTID_APP_USER_MODEL: Guid = Guid::from_bytes([
    0x55, 0x28, 0x4C, 0x9F, 0x79, 0x9F, 0x39, 0x4B, 0xA8, 0xD0, 0xE1, 0xD4, 0x2D, 0xE1, 0xD5, 0xF3,
]);

/// `System.AppUserModel.ID`, a `VT_LPWSTR`. Property 5 of
/// [`FMTID_APP_USER_MODEL`].
pub const PID_APP_USER_MODEL_ID: u32 = 5;
/// `System.AppUserModel.RelaunchCommand`, a `VT_LPWSTR`.
pub const PID_APP_USER_MODEL_RELAUNCH_COMMAND: u32 = 2;
/// `System.AppUserModel.RelaunchIconResource`, a `VT_LPWSTR`.
pub const PID_APP_USER_MODEL_RELAUNCH_ICON_RESOURCE: u32 = 3;
/// `System.AppUserModel.RelaunchDisplayNameResource`, a `VT_LPWSTR`.
pub const PID_APP_USER_MODEL_RELAUNCH_DISPLAY_NAME_RESOURCE: u32 = 4;
/// `System.AppUserModel.PreventPinning`, a `VT_BOOL`.
pub const PID_APP_USER_MODEL_PREVENT_PINNING: u32 = 9;

/// `PropertyType` values from MS-OLEPS 2.14 that this crate decodes.
pub mod vt {
    /// Zero bytes of value.
    pub const EMPTY: u16 = 0x0000;
    /// Zero bytes of value, and a different kind of nothing from `VT_EMPTY`.
    pub const NULL: u16 = 0x0001;
    /// 32-bit signed integer.
    pub const I4: u16 = 0x0003;
    /// A `CodePageString`.
    pub const BSTR: u16 = 0x0008;
    /// A `VARIANT_BOOL`, padded to four bytes.
    pub const BOOL: u16 = 0x000B;
    /// 32-bit unsigned integer.
    pub const UI4: u16 = 0x0013;
    /// A `UnicodeString`.
    pub const LPWSTR: u16 = 0x001F;
    /// A `FILETIME` (Packet Version).
    pub const FILETIME: u16 = 0x0040;
    /// A `GUID` (Packet Version).
    pub const CLSID: u16 = 0x0048;
}

/// `VARIANT_TRUE`, MS-OAUT 2.2.27. `VARIANT_FALSE` is `0x0000`.
pub const VARIANT_TRUE: u16 = 0xFFFF;

/// A serialized property store: the `PropertyStore` field of a
/// `PropertyStoreDataBlock`.
///
/// Borrows; parsing is deferred to the iterators, so constructing one cannot
/// fail and a malformed store still yields the storages before the break.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PropertyStore<'a> {
    buf: &'a [u8],
}

impl<'a> PropertyStore<'a> {
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    /// The undecoded bytes, exactly as they sat in the block.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.buf
    }

    /// Walk the property storages.
    #[must_use]
    pub const fn storages(&self) -> PropertyStorages<'a> {
        PropertyStorages::new(self.buf)
    }

    /// The first storage with this format ID.
    ///
    /// Stops at the first structural error, like the iterator: a store that
    /// breaks before the storage you asked for reads as "not present", because
    /// there is no honest way to distinguish the two.
    #[must_use]
    pub fn storage(&self, format_id: &Guid) -> Option<PropertyStorage<'a>> {
        self.storages()
            .map_while(Result::ok)
            .find(|s| s.format_id == *format_id)
    }

    /// One integer-named property, by format ID and property ID.
    #[must_use]
    pub fn get(&self, format_id: &Guid, id: u32) -> Option<PropertyValue<'a>> {
        self.storage(format_id)?.get(id)
    }

    /// `System.AppUserModel.ID`, if the link declares one.
    ///
    /// The string is whatever the application that wrote the shortcut chose. It
    /// is an identity claim, not a verified one: nothing stops a `.lnk` from
    /// naming another application's AppUserModelID, which is exactly how a
    /// shortcut gets itself grouped under someone else's taskbar button. Report
    /// it, do not trust it.
    #[must_use]
    pub fn app_user_model_id(&self) -> Option<ShellStr<'a>> {
        match self.get(&FMTID_APP_USER_MODEL, PID_APP_USER_MODEL_ID)? {
            PropertyValue::Lpwstr(s) | PropertyValue::Bstr(s) => Some(s),
            _ => None,
        }
    }
}

/// Iterator over the serialized property storages of a store.
///
/// Yields `Err` at most once: after a bad `StorageSize` the cursor is no longer
/// on a record boundary, so everything past it would be noise.
///
/// # Where a store ends
///
/// MS-PROPSTORE 2.1 requires a terminating storage whose `StorageSize` is zero,
/// and `BlockSize >= 0x0000000C` in MS-SHLLINK 2.5.7 leaves room for exactly
/// that and nothing else. Running out of buffer on a record boundary also ends
/// the walk cleanly, because a `PropertyStore` sliced out by a block size
/// frequently carries no terminator of its own. [`PropertyStorages::is_terminated`]
/// tells the two apart.
#[derive(Debug, Clone)]
pub struct PropertyStorages<'a> {
    r: Reader<'a>,
    done: bool,
    terminated: bool,
}

impl<'a> PropertyStorages<'a> {
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self {
            r: Reader::new(buf),
            done: false,
            terminated: false,
        }
    }

    /// `true` if the walk stopped on an explicit zero `StorageSize`.
    #[must_use]
    pub const fn is_terminated(&self) -> bool {
        self.terminated
    }

    /// Bytes consumed, terminator included.
    #[must_use]
    pub const fn bytes_consumed(&self) -> usize {
        self.r.pos()
    }
}

impl<'a> Iterator for PropertyStorages<'a> {
    type Item = Result<PropertyStorage<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // Fewer than four bytes left cannot be a size field. Zero left is the
        // normal end of a store cut to a block size; one to three is padding
        // nobody documented, and dropping it silently beats refusing the
        // storages already read.
        if self.r.remaining_len() < 4 {
            self.done = true;
            return None;
        }
        let at = self.r.pos();
        let size = match self.r.peek_u32_le() {
            Ok(v) => v,
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };
        if size == 0 {
            let _ = self.r.skip(4);
            self.terminated = true;
            self.done = true;
            return None;
        }
        if size < MIN_STORAGE_SIZE {
            self.done = true;
            return Some(Err(Error::new(ErrorKind::BadLength, at)));
        }
        let Ok(size_usize) = usize::try_from(size) else {
            self.done = true;
            return Some(Err(Error::new(ErrorKind::TooLarge, at)));
        };
        let mut inner = match self.r.take_reader(size_usize) {
            Ok(v) => v,
            Err(_) => {
                self.done = true;
                return Some(Err(Error::new(ErrorKind::BadLength, at)));
            }
        };
        let parsed = PropertyStorage::parse(&mut inner, size, at);
        if parsed.is_err() {
            self.done = true;
        }
        Some(parsed)
    }
}

impl core::iter::FusedIterator for PropertyStorages<'_> {}

/// One serialized property storage: a format ID and the values under it.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PropertyStorage<'a> {
    /// `StorageSize`, which counts itself.
    pub size: u32,
    /// `Version`. Always [`VERSION`]; a storage that says anything else is
    /// rejected before this struct exists.
    pub version: u32,
    /// The format ID. Compare against [`FMTID_APP_USER_MODEL`] and friends.
    pub format_id: Guid,
    /// Byte offset of this storage's `StorageSize` within the store.
    pub offset: usize,
    values: &'a [u8],
}

impl<'a> PropertyStorage<'a> {
    fn parse(r: &mut Reader<'a>, size: u32, at: usize) -> Result<Self> {
        r.skip(4)?; // StorageSize, already read
        let version = r.u32_le()?;
        if version != VERSION {
            // The one hard structural check in this module. Without it a
            // mis-sized outer block would be read as a storage whose format ID
            // is sixteen bytes of something else, and the values under a wrong
            // format ID are worse than no values.
            return Err(Error::new(ErrorKind::BadMagic, at + 4));
        }
        let format_id = Guid::from_bytes(r.guid()?);
        Ok(Self {
            size,
            version,
            format_id,
            offset: at,
            values: r.remaining(),
        })
    }

    /// `true` if this storage's values are named by string rather than by
    /// integer, i.e. its format ID is [`FMTID_STRING_NAMED`].
    #[must_use]
    pub fn is_string_named(&self) -> bool {
        self.format_id == FMTID_STRING_NAMED
    }

    /// The undecoded value region: everything after `FormatID`.
    #[must_use]
    pub const fn value_bytes(&self) -> &'a [u8] {
        self.values
    }

    /// Walk the properties.
    #[must_use]
    pub fn values(&self) -> Properties<'a> {
        Properties {
            r: Reader::new(self.values),
            base: self.offset + 24,
            string_named: self.is_string_named(),
            done: false,
        }
    }

    /// One integer-named property, by ID. Always `None` on a string-named
    /// storage.
    #[must_use]
    pub fn get(&self, id: u32) -> Option<PropertyValue<'a>> {
        self.values()
            .map_while(Result::ok)
            .find(|p| p.name == PropertyName::Integer(id))
            .map(|p| p.value)
    }

    /// One string-named property, by its name in UTF-16LE bytes.
    ///
    /// The name is compared as bytes because that is what is on the wire and
    /// because this crate does not allocate: a caller with a `&str` encodes it
    /// once and passes the buffer.
    #[must_use]
    pub fn get_named(&self, name_utf16le: &[u8]) -> Option<PropertyValue<'a>> {
        self.values()
            .map_while(Result::ok)
            .find(|p| match p.name {
                PropertyName::String(s) => s.as_bytes() == name_utf16le,
                PropertyName::Integer(_) => false,
            })
            .map(|p| p.value)
    }
}

/// How a property is named within its storage.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum PropertyName<'a> {
    /// The integer-named form (MS-PROPSTORE 2.3.2), which is everything except
    /// the one format ID in [`FMTID_STRING_NAMED`].
    Integer(u32),
    /// The string-named form (2.3.1). Always UTF-16LE, trimmed at its NUL, and
    /// not necessarily *valid* UTF-16 — it came off the wire.
    String(ShellStr<'a>),
}

/// One property: a name, a value, and the frame they arrived in.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Property<'a> {
    pub name: PropertyName<'a>,
    pub value: PropertyValue<'a>,
    /// `ValueSize`, which counts itself.
    pub size: u32,
    /// Byte offset of this property's `ValueSize` within the store.
    pub offset: usize,
}

/// Iterator over the properties of one storage.
///
/// Yields `Err` at most once, and only for a broken frame. A value whose
/// `VT_*` this crate does not decode is not an error — see
/// [`PropertyValue::Unsupported`].
#[derive(Debug, Clone)]
pub struct Properties<'a> {
    r: Reader<'a>,
    base: usize,
    string_named: bool,
    done: bool,
}

impl<'a> Properties<'a> {
    /// `true` if this storage's values are string-named.
    #[must_use]
    pub const fn is_string_named(&self) -> bool {
        self.string_named
    }
}

impl<'a> Iterator for Properties<'a> {
    type Item = Result<Property<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        if self.r.remaining_len() < 4 {
            self.done = true;
            return None;
        }
        let at = self.base + self.r.pos();
        let size = match self.r.peek_u32_le() {
            Ok(v) => v,
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };
        if size == 0 {
            let _ = self.r.skip(4);
            self.done = true;
            return None;
        }
        if size < MIN_VALUE_SIZE {
            self.done = true;
            return Some(Err(Error::new(ErrorKind::BadLength, at)));
        }
        let Ok(size_usize) = usize::try_from(size) else {
            self.done = true;
            return Some(Err(Error::new(ErrorKind::TooLarge, at)));
        };
        let mut inner = match self.r.take_reader(size_usize) {
            Ok(v) => v,
            Err(_) => {
                self.done = true;
                return Some(Err(Error::new(ErrorKind::BadLength, at)));
            }
        };
        let parsed = parse_property(&mut inner, self.string_named, size, at);
        if parsed.is_err() {
            self.done = true;
        }
        Some(parsed)
    }
}

impl core::iter::FusedIterator for Properties<'_> {}

fn parse_property<'a>(
    r: &mut Reader<'a>,
    string_named: bool,
    size: u32,
    at: usize,
) -> Result<Property<'a>> {
    r.skip(4)?; // ValueSize, already read
    let name = if string_named {
        let name_size = r.u32_le()?;
        // Reserved comes *before* the name, not after it: 2.3.1 lays out
        // ValueSize, NameSize, Reserved, Name. Reading it after the name is a
        // natural slip and shifts every value in the storage by one byte.
        r.skip(1)?;
        let Ok(name_size) = usize::try_from(name_size) else {
            return Err(Error::new(ErrorKind::TooLarge, at + 4));
        };
        let raw = r
            .take(name_size)
            .map_err(|_| Error::new(ErrorKind::BadLength, at + 4))?;
        PropertyName::String(ShellStr::Utf16(trim_utf16_nul(raw)))
    } else {
        let id = r.u32_le()?;
        r.skip(1)?;
        PropertyName::Integer(id)
    };

    let property_type = r.u16_le()?;
    // MS-OLEPS 2.15 says the two padding bytes "MUST be set to zero, and any
    // nonzero value SHOULD be rejected". SHOULD, not MUST, and rejecting costs
    // the whole storage; the field is read and dropped instead.
    r.skip(2)?;

    // Decode from a clone so the raw payload is still reachable when the type
    // is one this crate does not cover, or when it is one it does cover and the
    // payload is too short for it. Either way the value survives as bytes and
    // the walk goes on.
    let mut vr = r.clone();
    let value =
        PropertyValue::decode(property_type, &mut vr).unwrap_or(PropertyValue::Unsupported {
            property_type,
            data: r.remaining(),
        });

    Ok(Property {
        name,
        value,
        size,
        offset: at,
    })
}

/// A `TypedPropertyValue` (MS-OLEPS 2.15), for the types that turn up in a
/// `.lnk`.
///
/// The `VT_*` space is large and most of it never appears in a shell link, so
/// this covers nine types and refuses to guess at the rest. Refusing is the
/// point: a `VT_VECTOR | VT_LPWSTR` decoded as if it were a `VT_LPWSTR` yields
/// a plausible string that is not the property's value.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PropertyValue<'a> {
    /// `VT_EMPTY`. No value was ever set.
    Empty,
    /// `VT_NULL`. A value was set, to nothing.
    Null,
    /// `VT_I4`.
    I4(i32),
    /// `VT_UI4`.
    U4(u32),
    /// `VT_BOOL`. A `VARIANT_BOOL`: [`VARIANT_TRUE`] is `0xFFFF`, and this is
    /// `true` for any non-zero word rather than for `0xFFFF` alone, because
    /// writers that store `1` exist and reading those as `false` is worse than
    /// being liberal.
    Bool(bool),
    /// `VT_FILETIME`.
    FileTime(FileTime),
    /// `VT_CLSID`.
    Clsid(Guid),
    /// `VT_LPWSTR`: a `UnicodeString`, trimmed at its NUL.
    ///
    /// Note the unit: `UnicodeString`'s `Length` counts 16-bit characters
    /// including the terminator, so the byte count is twice it. Reading it as a
    /// byte count truncates every string to half its length.
    Lpwstr(ShellStr<'a>),
    /// `VT_BSTR`: a `CodePageString`, trimmed at its NUL.
    ///
    /// Returned as [`ShellStr::Ansi`] because `CodePageString`'s encoding is
    /// defined by the enclosing property set's `CodePage` property — and a
    /// serialized property *storage* has no such property. There is nothing in
    /// the payload that says what these bytes mean; in practice they are the
    /// writing machine's ANSI code page. Windows writes `VT_LPWSTR` here.
    Bstr(ShellStr<'a>),
    /// A well-formed record this crate did not decode: a `VT_*` outside the set
    /// above, or one of those with a payload too short for it.
    ///
    /// Not an error, and not a dead end — `data` is everything after the
    /// `TypedPropertyValue` header, so a caller that knows the type can decode
    /// it.
    Unsupported {
        /// The `Type` word as read.
        property_type: u16,
        /// The value payload, bounded by the enclosing `ValueSize`.
        data: &'a [u8],
    },
}

impl<'a> PropertyValue<'a> {
    /// Decode a `TypedPropertyValue` body, the `Type` word having been read.
    ///
    /// `r` must be bounded by the value's own `ValueSize` — [`Properties`] does
    /// that with a sub-reader, and a caller doing it by hand must too.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Unsupported`] for a `VT_*` outside the covered set, and
    /// [`ErrorKind::UnexpectedEof`] for a payload too short for the type it
    /// claims. [`Properties`] turns both into [`PropertyValue::Unsupported`]
    /// rather than letting either end the walk.
    pub fn decode(property_type: u16, r: &mut Reader<'a>) -> Result<Self> {
        Ok(match property_type {
            vt::EMPTY => Self::Empty,
            vt::NULL => Self::Null,
            vt::I4 => Self::I4(r.i32_le()?),
            vt::UI4 => Self::U4(r.u32_le()?),
            vt::BOOL => Self::Bool(r.u16_le()? != 0),
            vt::FILETIME => Self::FileTime(FileTime::read(r)?),
            vt::CLSID => Self::Clsid(Guid::from_bytes(r.guid()?)),
            vt::LPWSTR => {
                let units = usize::try_from(r.u32_le()?).map_err(|_| r.err(ErrorKind::TooLarge))?;
                let bytes = units
                    .checked_mul(2)
                    .ok_or_else(|| r.err(ErrorKind::TooLarge))?;
                Self::Lpwstr(ShellStr::Utf16(trim_utf16_nul(r.take(bytes)?)))
            }
            vt::BSTR => {
                let len = usize::try_from(r.u32_le()?).map_err(|_| r.err(ErrorKind::TooLarge))?;
                Self::Bstr(ShellStr::Ansi(trim_nul(r.take(len)?)))
            }
            _ => return Err(r.err(ErrorKind::Unsupported)),
        })
    }

    /// The `VT_*` tag this value was read as.
    #[must_use]
    pub const fn property_type(&self) -> u16 {
        match self {
            Self::Empty => vt::EMPTY,
            Self::Null => vt::NULL,
            Self::I4(_) => vt::I4,
            Self::U4(_) => vt::UI4,
            Self::Bool(_) => vt::BOOL,
            Self::FileTime(_) => vt::FILETIME,
            Self::Clsid(_) => vt::CLSID,
            Self::Lpwstr(_) => vt::LPWSTR,
            Self::Bstr(_) => vt::BSTR,
            Self::Unsupported { property_type, .. } => *property_type,
        }
    }

    /// The value as a string, for the two string types.
    #[must_use]
    pub const fn as_str(&self) -> Option<ShellStr<'a>> {
        match self {
            Self::Lpwstr(s) | Self::Bstr(s) => Some(*s),
            _ => None,
        }
    }

    /// The value as an unsigned integer, for the two integer types. `VT_I4`
    /// is reinterpreted, not clamped.
    #[must_use]
    pub const fn as_u32(&self) -> Option<u32> {
        match self {
            Self::U4(v) => Some(*v),
            Self::I4(v) => Some(*v as u32),
            _ => None,
        }
    }
}

/// Bytes up to the first NUL, or all of them if there is none.
fn trim_nul(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

/// The same for UTF-16LE, on even boundaries.
fn trim_utf16_nul(bytes: &[u8]) -> &[u8] {
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == 0 && bytes[i + 1] == 0 {
            return &bytes[..i];
        }
        i += 2;
    }
    bytes
}

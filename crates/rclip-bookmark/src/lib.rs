//! Reader for macOS `BookmarkData` — the `book` / `alis` blob that
//! `NSURL.bookmarkData` returns and that modern Finder alias files contain.
//!
//! # Why bother, when there is already a `file://` URL
//!
//! A bookmark keeps the target's catalog node ID, its volume's UUID, and the
//! path components separately. That is what lets macOS resolve it **after the
//! file has been moved or renamed** — the CNID still matches even though every
//! path component changed. A `file://` URL on the pasteboard breaks the moment
//! the user drags the file to another folder; a bookmark does not. When Finder
//! puts an alias on the pasteboard, this is what is in it.
//!
//! # Format
//!
//! Apple has never specified it. The layout below is the consensus of three
//! independent reverse-engineering efforts, cited in the crate README, and is
//! checked against bookmarks produced by CoreFoundation on macOS 15.
//!
//! ```text
//! header    magic 'book'|'alis' │ u32 total size │ u32 version │ u32 header size │ reserved
//! @hdrsize  u32 offset of the first TOC
//! TOC       u32 size-8 │ u32 0xFFFFFFFE │ u32 id │ u32 next TOC │ u32 count │ entries[]
//! entry     u32 key │ u32 record offset │ u32 reserved
//! record    u32 length │ u32 type │ payload, padded to 4 bytes
//! ```
//!
//! Two things trip up every first implementation:
//!
//! - **Every offset is relative to the end of the header**, not to the start of
//!   the buffer. And the header length is a *field*: it is 48 bytes today and
//!   64 on macOS 26, where the prolog grew a team identifier. Hardcoding 48
//!   works until it doesn't.
//! - **`0x0400` date records are big-endian** in an otherwise entirely
//!   little-endian format. Read one the wrong way round and you get a
//!   plausible-looking `f64` about 10^300 seconds from the epoch instead of a
//!   parse error, so nothing tells you.
//!
//! # Security
//!
//! Every offset in the format is attacker-controlled: the TOC chain, each
//! entry's record offset, and every element of every array and dictionary. All
//! of them are validated against the bookmark's declared extent before use, and
//! container traversal is charged against [`rclip_core::MAX_DEPTH`] so that an
//! offset pointing back at its own container terminates instead of recursing.
//! [`Bookmark::validate`] walks the whole graph under a node budget for callers
//! that want to know a blob is sound before handing it on.
//!
//! # Example
//!
//! ```
//! # fn main() -> Result<(), rclip_core::Error> {
//! # let bytes = include_bytes!("../../../corpus/synthetic/rclip-bookmark/url-and-filename.bin");
//! let bm = rclip_bookmark::Bookmark::parse(bytes)?;
//! assert_eq!(bm.target_filename()?, Some("report.pdf"));
//! # Ok(())
//! # }
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod flags;
pub mod key;
mod toc;
mod value;

use rclip_core::{Error, ErrorKind, Reader, Result, MAX_DEPTH};

pub use flags::Flags;
pub use toc::{Entry, EntryIter, EntryKey, Toc, TocIter, TOC_MAGIC};
pub use value::{
    number, ty, url, Array, ArrayIter, Date, Dict, DictIter, RelativeUrl, Value,
    CF_EPOCH_UNIX_SECS, SUBTYPE_MASK, TYPE_MASK,
};

/// Magic for a bookmark produced by `NSURL.bookmarkData`.
pub const MAGIC_BOOK: &[u8; 4] = b"book";
/// Magic for a bookmark stored in a Finder alias file. Byte-identical format;
/// only the four signature bytes differ.
pub const MAGIC_ALIS: &[u8; 4] = b"alis";

/// The version word seen on every bookmark CoreFoundation has written since
/// 10.6, at offset 8. macOS 26 bumped it to `0x10050000` and grew the prolog to
/// 64 bytes to carry a team identifier.
pub const VERSION_10040000: u32 = 0x1004_0000;

/// Smallest possible record: a length word, a type word, and no payload.
/// Referenced by the node budget in [`Bookmark::validate`].
pub const MIN_RECORD_LEN: usize = 8;

/// Which of the two signatures the bookmark carries.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Magic {
    /// `book` — pasteboard and `NSURL` bookmark data.
    Book,
    /// `alis` — the same structure inside a Finder alias file.
    Alis,
}

/// A parsed `BookmarkData` header, over a borrowed buffer.
///
/// Parsing validates the header and nothing else; TOCs, entries and records are
/// resolved on demand. That keeps `parse` cheap and means a malformed record
/// only costs the caller that asks for it. `Copy`, so it can be handed to the
/// lazy container types without lifetime gymnastics.
#[derive(Debug, Copy, Clone)]
pub struct Bookmark<'a> {
    /// The buffer truncated to the bookmark's *declared* size. Every read goes
    /// through this rather than the caller's slice, so a bookmark embedded in a
    /// larger blob cannot use an offset to read the bytes that follow it.
    data: &'a [u8],
    header_size: usize,
    magic: Magic,
    version: u32,
    first_toc: u32,
}

impl<'a> Bookmark<'a> {
    /// Parse the header of a `book` / `alis` blob.
    ///
    /// Trailing bytes after the declared size are tolerated — bookmarks arrive
    /// embedded in pasteboard items and alias files — but they are not
    /// reachable through any offset in the bookmark.
    pub fn parse(buf: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(buf);
        let magic = match r.take(4)? {
            b"book" => Magic::Book,
            b"alis" => Magic::Alis,
            _ => return Err(Error::new(ErrorKind::BadMagic, 0)),
        };
        let size = r.u32_le()? as usize;
        let version = r.u32_le()?;
        let header_size = r.u32_le()? as usize;

        // The header size is trusted as the offset base but not as a length:
        // it has to describe a header that actually fits inside the declared
        // bookmark, and leave room for the first-TOC pointer that follows it.
        if header_size < 16 {
            return Err(Error::new(ErrorKind::BadLength, 12));
        }
        if size > buf.len() {
            return Err(Error::new(ErrorKind::UnexpectedEof, buf.len()));
        }
        let payload_start = header_size
            .checked_add(4)
            .ok_or(Error::new(ErrorKind::TooLarge, 12))?;
        if payload_start > size {
            return Err(Error::new(ErrorKind::BadLength, 4));
        }

        let data = buf
            .get(..size)
            .ok_or(Error::new(ErrorKind::UnexpectedEof, size))?;
        let first_toc = Reader::new(data).peek_u32_le_at(header_size)?;

        Ok(Self {
            data,
            header_size,
            magic,
            version,
            first_toc,
        })
    }

    /// Which signature the blob carries.
    #[must_use]
    pub const fn magic(&self) -> Magic {
        self.magic
    }

    /// The version word. [`VERSION_10040000`] for everything CoreFoundation has
    /// written between 10.6 and macOS 15.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Length of the header, and therefore the base every offset in the file is
    /// measured from.
    #[must_use]
    pub const fn header_size(&self) -> usize {
        self.header_size
    }

    /// The bookmark's declared size in bytes.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.data.len()
    }

    /// Payload-relative offset of the first TOC.
    #[must_use]
    pub const fn first_toc_offset(&self) -> u32 {
        self.first_toc
    }

    pub(crate) const fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Iterate the TOC chain. Each item is fallible: a broken `next` pointer
    /// ends the chain with an error rather than truncating it silently.
    #[must_use]
    pub fn tocs(&self) -> TocIter<'a> {
        TocIter::new(self, self.first_toc)
    }

    /// Turn a payload-relative offset into an index into [`Bookmark::data`].
    ///
    /// This is the choke point every attacker-controlled offset in the format
    /// passes through. It exists so that no other function in the crate has to
    /// remember that offsets are measured from the end of the header.
    pub(crate) fn abs(&self, rel: u32) -> Result<usize> {
        let abs = self
            .header_size
            .checked_add(rel as usize)
            .ok_or(Error::new(ErrorKind::TooLarge, self.header_size))?;
        if abs >= self.data.len() {
            return Err(Error::new(ErrorKind::BadOffset, abs));
        }
        Ok(abs)
    }

    /// Decode the record at payload-relative offset `rel`.
    ///
    /// `depth` is how many containers were opened to get here. Containers hand
    /// their elements `depth + 1`, so a cyclic offset costs one level per hop
    /// and stops at [`rclip_core::MAX_DEPTH`] instead of running forever.
    pub(crate) fn value_at(&self, rel: u32, depth: u32) -> Result<Value<'a>> {
        let at = self.abs(rel)?;
        if depth >= MAX_DEPTH {
            return Err(Error::new(ErrorKind::DepthLimit, at));
        }

        let mut r = Reader::new(self.data);
        r.seek(at)?;
        let len = r.u32_le()? as usize;
        let type_code = r.u32_le()?;
        let payload_at = r.pos();
        let data = r.take(len)?;

        let subtype = type_code & SUBTYPE_MASK;
        match type_code & TYPE_MASK {
            ty::STRING => core::str::from_utf8(data)
                .map(Value::Str)
                .map_err(|_| Error::new(ErrorKind::InvalidUtf8, payload_at)),
            ty::DATA => Ok(Value::Data(data)),
            ty::NUMBER => number_value(data, subtype, type_code)
                .map_err(|_| Error::new(ErrorKind::BadLength, at)),
            ty::DATE => {
                // The one big-endian field in the format. `Reader::f64_be`
                // exists for exactly this record. The payload sub-reader counts
                // from the payload, so its errors are re-pointed at the record
                // to keep every offset this crate returns an index into the
                // caller's buffer.
                let mut d = Reader::new(data);
                let secs = d
                    .f64_be()
                    .map_err(|_| Error::new(ErrorKind::BadLength, at))?;
                Ok(Value::Date(Date::from_absolute_seconds(secs)))
            }
            ty::BOOLEAN => Ok(Value::Bool(subtype != 0)),
            ty::ARRAY => {
                // Length has to be a whole number of 4-byte offsets; a trailing
                // partial offset means the record was built or truncated wrong,
                // and silently dropping it would hide that.
                if len % 4 != 0 {
                    return Err(Error::new(ErrorKind::BadLength, at));
                }
                Ok(Value::Array(Array {
                    bm: *self,
                    offsets: data,
                    at,
                    depth,
                }))
            }
            ty::DICT => {
                if len % 8 != 0 {
                    return Err(Error::new(ErrorKind::BadLength, at));
                }
                Ok(Value::Dict(Dict {
                    bm: *self,
                    offsets: data,
                    at,
                    depth,
                }))
            }
            ty::UUID => {
                let bytes: [u8; 16] = data
                    .try_into()
                    .map_err(|_| Error::new(ErrorKind::BadLength, at))?;
                Ok(Value::Uuid(bytes))
            }
            ty::URL => match subtype {
                url::ABSOLUTE => core::str::from_utf8(data)
                    .map(Value::Url)
                    .map_err(|_| Error::new(ErrorKind::InvalidUtf8, payload_at)),
                url::RELATIVE => {
                    let mut u = Reader::new(data);
                    let mut word = || u.u32_le().map_err(|_| Error::new(ErrorKind::BadLength, at));
                    let base_offset = word()?;
                    let relative_offset = word()?;
                    Ok(Value::RelativeUrl(RelativeUrl {
                        bm: *self,
                        base_offset,
                        relative_offset,
                        at,
                        depth,
                    }))
                }
                _ => Ok(Value::Unknown { type_code, data }),
            },
            ty::NULL => Ok(Value::Null),
            _ => Ok(Value::Unknown { type_code, data }),
        }
    }

    /// The first value stored under numeric `key`, searching every TOC in the
    /// chain in order.
    ///
    /// Returns `Ok(None)` when the key is simply absent, and `Err` when the
    /// bookmark is malformed — the two are worth distinguishing, because half
    /// the keys here are optional in practice.
    pub fn get(&self, key: u32) -> Result<Option<Value<'a>>> {
        for toc in self.tocs() {
            for entry in toc?.iter() {
                let entry = entry?;
                if !entry.has_named_key() && entry.raw_key() == key {
                    return Ok(Some(entry.value()?));
                }
            }
        }
        Ok(None)
    }

    /// The first value stored under a string key of the given name.
    pub fn get_named(&self, name: &str) -> Result<Option<Value<'a>>> {
        for toc in self.tocs() {
            for entry in toc?.iter() {
                let entry = entry?;
                if entry.has_named_key() && entry.key()? == EntryKey::Named(name) {
                    return Ok(Some(entry.value()?));
                }
            }
        }
        Ok(None)
    }

    fn get_str(&self, key: u32) -> Result<Option<&'a str>> {
        Ok(self.get(key)?.and_then(|v| v.as_str()))
    }

    /// Target URL, key `0x1003`.
    ///
    /// Usually absent: CoreFoundation describes file targets with
    /// [`Bookmark::path_components`] instead and only stores a URL for targets
    /// that are not plain files.
    pub fn target_url(&self) -> Result<Option<&'a str>> {
        self.get_str(key::TARGET_URL)
    }

    /// Target file name, key `0x1020`.
    pub fn target_filename(&self) -> Result<Option<&'a str>> {
        self.get_str(key::TARGET_FILENAME)
    }

    /// Localised display name, key `0xF017`. Can differ from
    /// [`Bookmark::target_filename`] — a `.app` shows without its extension.
    pub fn display_name(&self) -> Result<Option<&'a str>> {
        self.get_str(key::DISPLAY_NAME)
    }

    /// Volume display name, key `0x2010` — `"Macintosh HD"` and friends.
    pub fn volume_name(&self) -> Result<Option<&'a str>> {
        self.get_str(key::VOLUME_NAME)
    }

    /// Mount path of the target's volume, key `0x2002`.
    pub fn volume_path(&self) -> Result<Option<&'a str>> {
        self.get_str(key::VOLUME_PATH)
    }

    /// Volume UUID, key `0x2011` — stored as a string, not as a UUID record.
    pub fn volume_uuid(&self) -> Result<Option<&'a str>> {
        self.get_str(key::VOLUME_UUID)
    }

    /// The target's path components, key `0x1004`: root first, separators not
    /// included, so `/private/tmp/x.txt` arrives as three items.
    pub fn path_components(&self) -> Result<Option<PathComponents<'a>>> {
        match self.get(key::TARGET_PATH)? {
            Some(Value::Array(a)) => Ok(Some(PathComponents {
                inner: a.iter(),
                at: a.offset(),
            })),
            Some(_) => Err(Error::new(ErrorKind::Malformed, self.header_size)),
            None => Ok(None),
        }
    }

    /// The target's creation date, key `0x1040`.
    pub fn target_creation_date(&self) -> Result<Option<Date>> {
        Ok(self
            .get(key::TARGET_CREATION_DATE)?
            .and_then(|v| v.as_date()))
    }

    /// When the bookmark itself was made, key `0xF030`.
    ///
    /// Stored as a plain little-endian `float64` rather than as a `0x0400`
    /// date, so it goes through [`Value::as_f64`], not [`Value::as_date`] — but
    /// the epoch is the same, so it comes back as a [`Date`] all the same.
    pub fn creation_time(&self) -> Result<Option<Date>> {
        Ok(self.get(key::CREATION_TIME)?.and_then(|v| {
            v.as_f64()
                .map(Date::from_absolute_seconds)
                .or_else(|| v.as_date())
        }))
    }

    /// `CFURL` resource property flags for the target, key `0x1010`.
    pub fn target_flags(&self) -> Result<Option<Flags>> {
        match self.get(key::TARGET_FLAGS)? {
            Some(Value::Data(d)) => Flags::parse(d).map(Some),
            _ => Ok(None),
        }
    }

    /// `CFURL` volume property flags, key `0x2020`.
    pub fn volume_flags(&self) -> Result<Option<Flags>> {
        match self.get(key::VOLUME_FLAGS)? {
            Some(Value::Data(d)) => Flags::parse(d).map(Some),
            _ => Ok(None),
        }
    }

    /// The sandbox extension token, key `0xF080` (read-write) or `0xF081`
    /// (read-only).
    ///
    /// Handed back as opaque bytes on purpose. It is a capability token —
    /// semicolon-separated fields plus an HMAC keyed to the machine and boot —
    /// and this crate has no business interpreting or reissuing one.
    pub fn sandbox_extension(&self) -> Result<Option<&'a [u8]>> {
        if let Some(v) = self.get(key::SANDBOX_RW_EXTENSION)? {
            return Ok(v.as_data());
        }
        Ok(self
            .get(key::SANDBOX_RO_EXTENSION)?
            .and_then(|v| v.as_data()))
    }

    /// Walk every TOC, entry and record, resolving the whole object graph.
    ///
    /// Nothing else in this crate does this — the reader is lazy so that a
    /// caller pays only for what it reads. Call this when you want to know a
    /// blob is structurally sound before storing it or passing it on.
    ///
    /// Two separate guards make the walk terminate on hostile input, because
    /// there are two separate shapes of attack. Depth is bounded by
    /// [`rclip_core::MAX_DEPTH`], which stops a container that points at
    /// itself. A *node budget* bounds the total number of records visited,
    /// which stops the other shape: eight nested arrays of eight shared
    /// references each fit in 400 bytes, nest only eight deep, and still cost
    /// 8^8 resolutions to walk. No record is smaller than [`MIN_RECORD_LEN`]
    /// bytes, so one visit per byte of payload leaves a comfortable eightfold
    /// margin for legitimately shared subtrees while keeping the total work
    /// linear in the input.
    pub fn validate(&self) -> Result<()> {
        let mut budget = self.data.len() + 1;
        for toc in self.tocs() {
            let toc = toc?;
            for entry in toc.iter() {
                let entry = entry?;
                entry.key()?;
                let value = entry.value()?;
                visit(&value, entry.offset(), &mut budget)?;
            }
        }
        Ok(())
    }

    /// Reconstruct the target's absolute path from [`Bookmark::path_components`].
    ///
    /// A reconstruction, not a resolution: nothing here touches the filesystem,
    /// follows a symlink, or checks that the path exists. The volume the
    /// components are rooted on is [`Bookmark::volume_path`], which is `/` for
    /// the boot volume and `/Volumes/<name>` otherwise.
    #[cfg(feature = "alloc")]
    pub fn target_path(&self) -> Result<Option<alloc::string::String>> {
        use alloc::string::String;

        let Some(components) = self.path_components()? else {
            return Ok(None);
        };
        let mut out = String::new();
        for c in components {
            out.push('/');
            out.push_str(c?);
        }
        if out.is_empty() {
            out.push('/');
        }
        Ok(Some(out))
    }
}

/// Recursive half of [`Bookmark::validate`].
///
/// Its own recursion needs no depth counter: every nested value came out of
/// `value_at(_, depth + 1)`, so the resolver has already refused anything past
/// [`rclip_core::MAX_DEPTH`] before this function can descend into it.
fn visit(value: &Value<'_>, at: usize, budget: &mut usize) -> Result<()> {
    if *budget == 0 {
        return Err(Error::new(ErrorKind::TooLarge, at));
    }
    *budget -= 1;

    match value {
        Value::Array(a) => {
            for element in a.iter() {
                visit(&element?, a.offset(), budget)?;
            }
        }
        Value::Dict(d) => {
            for pair in d.iter() {
                let (k, v) = pair?;
                visit(&k, d.offset(), budget)?;
                visit(&v, d.offset(), budget)?;
            }
        }
        Value::RelativeUrl(u) => {
            visit(&u.base()?, u.offset(), budget)?;
            visit(&u.relative()?, u.offset(), budget)?;
        }
        _ => {}
    }
    Ok(())
}

/// Decode a `0x03xx` number record according to its `CFNumberType` subtype.
///
/// Mother's Ruin reads `0x0303` / `0x0304` as *unsigned*; `mac_alias` and the
/// `CFNumberType` enum they come from say signed, so signed is what this
/// returns. It matters for `0x2012` volume size on a volume bigger than 8 EiB,
/// and nowhere else.
fn number_value<'a>(data: &'a [u8], subtype: u32, type_code: u32) -> Result<Value<'a>> {
    let mut r = Reader::new(data);
    Ok(match subtype {
        number::SINT8 => Value::I8(r.i8()?),
        number::SINT16 => Value::I16(r.i16_le()?),
        number::SINT32 => Value::I32(r.i32_le()?),
        number::SINT64 => Value::I64(r.i64_le()?),
        number::FLOAT32 => Value::F32(f32::from_bits(r.u32_le()?)),
        number::FLOAT64 => Value::F64(r.f64_le()?),
        // CFNumberType also defines 7..=16 (char, short, int, long, CFIndex,
        // CGFloat …), but CoreFoundation normalises to the fixed-width types
        // above when it encodes, so nothing in the corpus uses them.
        // TODO(phase-4): decode the remaining CFNumberType subtypes if a real
        // capture ever turns one up.
        _ => Value::Unknown { type_code, data },
    })
}

/// Iterator over the path components of key `0x1004`.
///
/// Each item is fallible because the array holds offsets, and any one of them
/// can be wrong.
#[derive(Debug, Clone)]
pub struct PathComponents<'a> {
    inner: ArrayIter<'a>,
    at: usize,
}

impl<'a> Iterator for PathComponents<'a> {
    type Item = Result<&'a str>;

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.inner.next()?;
        // A path component that is not a string means the array is not a path,
        // which is a structural problem with the array rather than with the
        // element — so the error points at the array record.
        Some(value.and_then(|v| v.as_str().ok_or(Error::new(ErrorKind::Malformed, self.at))))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for PathComponents<'_> {}

//! The Macintosh resource fork, enough of it to read an internet location file.
//!
//! Before Mac OS X a `.webloc` had an **empty data fork** and kept its URL in
//! the resource fork, as a `url ` resource. Finder still writes those resources
//! today alongside the plist: the capture in
//! `corpus/macos/finder/webloc-resource-fork.bin` is the resource fork of the
//! same file whose data fork is `corpus/synthetic/rclip-webloc/finder-created.bin`,
//! and it holds `drag`, `TEXT` and `url ` resources.
//!
//! A resource fork is not in the file's bytes. On macOS it is the
//! `<file>/..namedfork/rsrc` stream; in an archive it is an AppleDouble
//! sidecar; on the clipboard it does not travel at all. Getting hold of one is
//! the caller's problem — this module parses the bytes once you have them.
//!
//! # Layout
//!
//! From *Inside Macintosh: More Macintosh Toolbox*, "Resource File Format",
//! cross-checked against the Kaitai Struct `resource_fork.ksy` and against the
//! captured fork byte for byte. **Everything is big-endian**, like the binary
//! plist and unlike every Win32 structure in this workspace.
//!
//! ```text
//! header   16 bytes at 0:  u32 data_offset │ u32 map_offset
//!                          u32 data_len    │ u32 map_len
//!          then 112 bytes reserved for the system and 128 for the application
//!
//! data     at data_offset, data_len bytes: one block per resource,
//!          u32 length followed by that many bytes
//!
//! map      at map_offset, map_len bytes:
//!            +0   16  reserved copy of the header
//!            +16   4  reserved handle to the next map in memory
//!            +20   2  reserved file reference number
//!            +22   2  resource fork attributes
//!            +24   2  offset from the map to the type list
//!            +26   2  offset from the map to the name list
//!
//! type     at the type list offset:
//! list       u16 number of types MINUS ONE, then 8 bytes per type:
//!            4 type code │ u16 count minus one │ u16 offset to its
//!            reference list, measured from the start of the type list
//!
//! ref      12 bytes per resource:
//! list       i16 id │ u16 name offset (0xFFFF = unnamed) │ u8 attributes
//!            │ u24 offset into the data area │ u32 reserved handle
//!
//! names    Pascal strings: one length byte, then that many bytes
//! ```
//!
//! The two *minus one* counts are the trap. A type list that says `0` holds one
//! type, and a list that says `0xFFFF` holds none at all rather than 65 536 —
//! wrapping, not saturating, which is what the Resource Manager does and what
//! makes an empty fork expressible.
//!
//! # Security
//!
//! Every offset here is attacker-controlled: the header's four, the map's two,
//! one per type, one per resource for its name and one for its data. None is
//! used as an index. The header's ranges are checked against the real length
//! once at [`ResourceFork::parse`], the map's are checked against the map, and
//! the per-resource ones go through [`rclip_core::Reader`] like everything else
//! in this workspace. Nothing recurses, because the format does not nest.

use rclip_core::{Error, ErrorKind, Reader, Result};

/// Fixed size of the resource header.
pub const HEADER_LEN: usize = 16;

/// Smallest a resource map can be: 28 bytes of map header plus the two-byte
/// count that opens the type list.
pub const MIN_MAP_LEN: usize = 30;

/// Bytes per entry in the type list.
const TYPE_ENTRY_LEN: usize = 8;
/// Bytes per entry in a reference list.
const REF_ENTRY_LEN: usize = 12;

/// Offset of the type list offset, within the map.
const MAP_TYPE_LIST_OFFSET: usize = 24;
/// Offset of the name list offset, within the map.
const MAP_NAME_LIST_OFFSET: usize = 26;

/// `url ` — the URL of an internet location file. Note the trailing space:
/// resource types are exactly four characters.
pub const TYPE_URL: [u8; 4] = *b"url ";
/// `TEXT` — plain text. Finder writes the URL a second time under this type so
/// that dragging the file somewhere that wants text produces something useful.
pub const TYPE_TEXT: [u8; 4] = *b"TEXT";
/// `drag` — the Drag Manager flavor list naming the types above.
pub const TYPE_DRAG: [u8; 4] = *b"drag";

/// A parsed resource fork: the header, and a validated view of the map.
#[derive(Debug, Copy, Clone)]
pub struct ResourceFork<'a> {
    /// The resource data area, `data_len` bytes at `data_offset`.
    data: &'a [u8],
    /// The resource map, `map_len` bytes at `map_offset`.
    map: &'a [u8],
    /// Offset of the type list within [`Self::map`].
    type_list: usize,
    /// Offset of the name list within [`Self::map`].
    name_list: usize,
    /// Number of resource types, already un-decremented.
    num_types: usize,
}

impl<'a> ResourceFork<'a> {
    /// `true` if `buf` looks like a resource fork.
    ///
    /// A resource fork has no magic number — it opens with four offsets — so
    /// this is the header check and nothing more: both sections have to start
    /// after the header and lie inside the buffer, and the map has to be big
    /// enough to be a map. That is a weak-looking rule and a strong test in
    /// practice, because the first four bytes of any text are a `data_offset`
    /// in the hundreds of millions.
    ///
    /// Deliberately *not* [`ResourceFork::parse`] run for its verdict.
    /// "Is this a resource fork" and "is this resource fork well formed" are
    /// different questions, and collapsing them turns every structural error
    /// inside the map into `BadMagic` — which tells a caller the file is some
    /// other format when in fact it is this one, broken.
    #[must_use]
    pub fn detect(buf: &[u8]) -> bool {
        header(buf).is_ok()
    }

    /// Parse the header and the resource map.
    ///
    /// Resource *data* is not touched here; a block whose length field runs
    /// past the data area costs only the resource that names it.
    pub fn parse(buf: &'a [u8]) -> Result<Self> {
        let (data, map) = header(buf)?;
        let map_len = map.len();

        let m = Reader::new(map);
        let type_list = usize::from(be_u16_at(&m, MAP_TYPE_LIST_OFFSET)?);
        let name_list = usize::from(be_u16_at(&m, MAP_NAME_LIST_OFFSET)?);

        // The type list opens with its own count, so it needs two bytes inside
        // the map before any entry is read.
        let count_at = type_list
            .checked_add(2)
            .ok_or(Error::new(ErrorKind::TooLarge, MAP_TYPE_LIST_OFFSET))?;
        if count_at > map_len || name_list > map_len {
            return Err(Error::new(ErrorKind::BadOffset, MAP_TYPE_LIST_OFFSET));
        }

        // Wrapping, not saturating: 0xFFFF means no types at all. The Resource
        // Manager stores the count minus one and an empty fork has to be
        // expressible.
        let num_types = usize::from(be_u16_at(&m, type_list)?.wrapping_add(1));

        // Every type entry has to be inside the map before any of them is read,
        // so that a count off the wire cannot drive a walk past the end.
        let mut entries = Reader::new(map);
        entries.seek(count_at)?;
        entries.check_count(num_types, TYPE_ENTRY_LEN)?;

        Ok(Self {
            data,
            map,
            type_list,
            name_list,
            num_types,
        })
    }

    /// The resource data area.
    #[must_use]
    pub const fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The resource map.
    #[must_use]
    pub const fn map(&self) -> &'a [u8] {
        self.map
    }

    /// How many resource types the fork declares.
    #[must_use]
    pub const fn type_count(&self) -> usize {
        self.num_types
    }

    /// Walk the type list.
    #[must_use]
    pub const fn types(&self) -> Types<'a> {
        Types {
            fork: *self,
            index: 0,
        }
    }

    /// The entry for one four-character type code, if the fork has it.
    #[must_use]
    pub fn find_type(&self, code: [u8; 4]) -> Option<ResourceType<'a>> {
        self.types().find(|t| t.code == code)
    }

    /// Walk the resources of one type. Empty when the fork has no such type.
    #[must_use]
    pub fn resources(&self, code: [u8; 4]) -> Resources<'a> {
        self.find_type(code).map_or(
            Resources {
                fork: *self,
                at: 0,
                remaining: 0,
            },
            |t| t.resources(),
        )
    }

    /// The first resource of a type, in reference-list order.
    ///
    /// Reference lists are sorted by resource ID, so "first" means lowest ID.
    /// An internet location file carries exactly one `url ` resource, so this
    /// is the whole lookup for the common case.
    pub fn first_resource(&self, code: [u8; 4]) -> Option<Result<Resource<'a>>> {
        self.resources(code).next()
    }

    /// Resolve a resource's data block: a `u32` length followed by that many
    /// bytes, at `offset` in the data area.
    fn data_block(&self, offset: usize) -> Result<&'a [u8]> {
        let mut r = Reader::new(self.data);
        r.seek(offset)?;
        let len = be_u32(&mut r)?;
        r.take(len)
    }

    /// Resolve a resource's name: a Pascal string at `offset` in the name list.
    fn name_at(&self, offset: usize) -> Result<&'a [u8]> {
        let at = self
            .name_list
            .checked_add(offset)
            .ok_or(Error::new(ErrorKind::TooLarge, MAP_NAME_LIST_OFFSET))?;
        let mut r = Reader::new(self.map);
        r.seek(at)?;
        let len = usize::from(r.u8()?);
        r.take(len)
    }
}

/// One entry in the type list.
#[derive(Debug, Copy, Clone)]
pub struct ResourceType<'a> {
    /// The four-character type code, e.g. [`TYPE_URL`].
    pub code: [u8; 4],
    /// How many resources of this type the fork holds. Never zero in a
    /// well-formed fork; the Resource Manager does not write empty lists.
    pub count: usize,
    /// Offset of this type's reference list, from the start of the type list.
    pub reference_list_offset: usize,
    fork: ResourceFork<'a>,
}

impl<'a> ResourceType<'a> {
    /// Walk this type's resources.
    #[must_use]
    pub fn resources(&self) -> Resources<'a> {
        // A reference list offset that does not resolve yields an empty walk
        // rather than an error, because the offset belongs to the *type* and
        // the iterator's items are resources. The type is still reported by
        // `ResourceFork::types`, so nothing is silently dropped.
        let at = self.fork.type_list.checked_add(self.reference_list_offset);
        let fits = at.and_then(|a| a.checked_add(self.count.checked_mul(REF_ENTRY_LEN)?));
        match (at, fits) {
            (Some(at), Some(end)) if end <= self.fork.map.len() => Resources {
                fork: self.fork,
                at,
                remaining: self.count,
            },
            _ => Resources {
                fork: self.fork,
                at: 0,
                remaining: 0,
            },
        }
    }
}

/// Iterator over a fork's type list. See [`ResourceFork::types`].
#[derive(Debug, Clone)]
pub struct Types<'a> {
    fork: ResourceFork<'a>,
    index: usize,
}

impl<'a> Iterator for Types<'a> {
    type Item = ResourceType<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.fork.num_types {
            return None;
        }
        // Infallible: `ResourceFork::parse` checked that every entry is inside
        // the map before handing the fork back.
        let at = self.fork.type_list + 2 + self.index * TYPE_ENTRY_LEN;
        let e = self.fork.map.get(at..at + TYPE_ENTRY_LEN)?;
        self.index += 1;
        Some(ResourceType {
            code: [e[0], e[1], e[2], e[3]],
            count: usize::from(u16::from_be_bytes([e[4], e[5]]).wrapping_add(1)),
            reference_list_offset: usize::from(u16::from_be_bytes([e[6], e[7]])),
            fork: self.fork,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.fork.num_types.saturating_sub(self.index);
        (n, Some(n))
    }
}

impl ExactSizeIterator for Types<'_> {}
impl core::iter::FusedIterator for Types<'_> {}

/// One resource: what it is, and the bytes it holds.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Resource<'a> {
    /// Resource ID. Signed, and negative IDs are ordinary.
    pub id: i16,
    /// Attribute bits — purgeable, locked, protected, preload, compressed.
    /// Returned raw: none of them changes how the data is read here, and
    /// `resCompressed` in particular describes a compression scheme this crate
    /// does not implement, so a caller that sees it should not trust the bytes
    /// to be the resource's value.
    pub attributes: u8,
    /// The resource's name, when it has one. Bytes rather than a `&str`:
    /// resource names are in the system encoding of whatever machine wrote
    /// them, and the fork does not record which one that was.
    pub name: Option<&'a [u8]>,
    /// The resource's data.
    pub data: &'a [u8],
}

/// `resCompressed`, bit 0 of a resource's attributes.
pub const ATTR_COMPRESSED: u8 = 0x01;

impl Resource<'_> {
    /// `true` if the resource is marked compressed, in which case
    /// [`Resource::data`] is a compressed image and not the resource's value.
    #[must_use]
    pub const fn is_compressed(&self) -> bool {
        self.attributes & ATTR_COMPRESSED != 0
    }
}

/// Iterator over one type's resources. See [`ResourceFork::resources`].
#[derive(Debug, Clone)]
pub struct Resources<'a> {
    fork: ResourceFork<'a>,
    /// Offset of the next reference entry within the map.
    at: usize,
    remaining: usize,
}

impl<'a> Iterator for Resources<'a> {
    type Item = Result<Resource<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let at = self.at;
        self.at += REF_ENTRY_LEN;
        Some(self.fork.reference(at))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for Resources<'_> {}
impl core::iter::FusedIterator for Resources<'_> {}

impl<'a> ResourceFork<'a> {
    /// Read one reference-list entry and resolve what it points at.
    fn reference(&self, at: usize) -> Result<Resource<'a>> {
        let mut r = Reader::new(self.map);
        r.seek(at)?;
        let e = r.take(REF_ENTRY_LEN)?;
        let id = i16::from_be_bytes([e[0], e[1]]);
        let name_offset = u16::from_be_bytes([e[2], e[3]]);
        let attributes = e[4];
        // The data offset is three bytes packed against the attribute byte.
        let data_offset = usize::from(e[5]) << 16 | usize::from(e[6]) << 8 | usize::from(e[7]);

        // 0xFFFF is -1 truncated: the resource has no name. A name that does
        // not resolve is not fatal — the resource's data is what a caller came
        // for, and a broken name list should not cost it.
        let name = if name_offset == u16::MAX {
            None
        } else {
            self.name_at(usize::from(name_offset)).ok()
        };

        Ok(Resource {
            id,
            attributes,
            name,
            data: self.data_block(data_offset)?,
        })
    }
}

/// Validate the 16-byte header and borrow the two sections it names.
///
/// Split out from [`ResourceFork::parse`] because [`ResourceFork::detect`]
/// needs exactly this much and no more.
fn header(buf: &[u8]) -> Result<(&[u8], &[u8])> {
    let r = Reader::new(buf);
    let mut h = Reader::new(r.slice_at(0, HEADER_LEN)?);
    let data_offset = be_u32(&mut h)?;
    let map_offset = be_u32(&mut h)?;
    let data_len = be_u32(&mut h)?;
    let map_len = be_u32(&mut h)?;

    // A fork that puts either section inside the header is not one; the first
    // sixteen bytes are the header by definition.
    if data_offset < HEADER_LEN || map_offset < HEADER_LEN {
        return Err(Error::new(ErrorKind::BadOffset, 0));
    }
    if map_len < MIN_MAP_LEN {
        return Err(Error::new(ErrorKind::BadLength, 12));
    }
    Ok((
        r.slice_at(data_offset, data_len)?,
        r.slice_at(map_offset, map_len)?,
    ))
}

/// Read a big-endian `u32` as a `usize`.
///
/// Every length and offset in this format is a `u32`, and on a 16-bit target
/// `usize` is narrower, so the conversion is checked rather than cast. A value
/// that does not fit cannot index the buffer either, so `TooLarge` is the
/// honest answer.
fn be_u32(r: &mut Reader<'_>) -> Result<usize> {
    let at = r.pos();
    let b = r.take(4)?;
    usize::try_from(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .map_err(|_| Error::new(ErrorKind::TooLarge, at))
}

/// Read a big-endian `u16` at an absolute offset, without moving the cursor.
///
/// Stays a `u16` because two fields in this format are *counts minus one* and
/// have to wrap at 16 bits: widening first would turn `0xFFFF` into 65 536
/// types instead of none.
fn be_u16_at(r: &Reader<'_>, at: usize) -> Result<u16> {
    let b = r.slice_at(at, 2)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest legal fork: a header, an empty data area, and a map whose
    /// type list says `0xFFFF` — no types at all.
    fn empty() -> [u8; HEADER_LEN + MIN_MAP_LEN] {
        let mut b = [0u8; HEADER_LEN + MIN_MAP_LEN];
        let map_offset = HEADER_LEN as u32;
        b[0..4].copy_from_slice(&map_offset.to_be_bytes()); // data offset
        b[4..8].copy_from_slice(&map_offset.to_be_bytes()); // map offset
        b[8..12].copy_from_slice(&0u32.to_be_bytes()); // data length
        b[12..16].copy_from_slice(&(MIN_MAP_LEN as u32).to_be_bytes());
        // Map: type list at +28, name list at the end of the map.
        b[HEADER_LEN + MAP_TYPE_LIST_OFFSET..][..2].copy_from_slice(&28u16.to_be_bytes());
        b[HEADER_LEN + MAP_NAME_LIST_OFFSET..][..2]
            .copy_from_slice(&(MIN_MAP_LEN as u16).to_be_bytes());
        b[HEADER_LEN + 28..][..2].copy_from_slice(&u16::MAX.to_be_bytes());
        b
    }

    #[test]
    fn a_count_of_ffff_means_no_types_and_not_sixty_five_thousand() {
        let b = empty();
        let fork = ResourceFork::parse(&b).expect("well formed");
        assert_eq!(fork.type_count(), 0);
        assert_eq!(fork.types().count(), 0);
        assert!(fork.first_resource(TYPE_URL).is_none());
    }

    #[test]
    fn a_map_shorter_than_its_own_header_is_rejected() {
        let mut b = empty();
        b[12..16].copy_from_slice(&(MIN_MAP_LEN as u32 - 1).to_be_bytes());
        assert_eq!(
            ResourceFork::parse(&b).unwrap_err().kind,
            ErrorKind::BadLength
        );
    }

    #[test]
    fn a_section_overlapping_the_header_is_rejected() {
        for field in [0usize, 4] {
            let mut b = empty();
            b[field..field + 4].copy_from_slice(&4u32.to_be_bytes());
            assert_eq!(
                ResourceFork::parse(&b).unwrap_err().kind,
                ErrorKind::BadOffset,
                "field at {field}"
            );
        }
    }

    #[test]
    fn a_type_count_that_would_walk_past_the_map_is_rejected() {
        let mut b = empty();
        // 0xFFFE + 1 = 0xFFFF types, times eight bytes each.
        b[HEADER_LEN + 28..][..2].copy_from_slice(&(u16::MAX - 1).to_be_bytes());
        assert_eq!(
            ResourceFork::parse(&b).unwrap_err().kind,
            ErrorKind::TooLarge
        );
    }

    #[test]
    fn text_is_never_mistaken_for_a_resource_fork() {
        for s in [
            &b"<?xml version=\"1.0\"?><plist><dict></dict></plist>"[..],
            b"bplist00",
            b"https://example.com/",
            b"",
        ] {
            assert!(!ResourceFork::detect(s), "{s:?}");
        }
    }
}

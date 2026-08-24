//! Just enough of the `bplist00` binary property list format to get one string
//! out of the root dictionary.
//!
//! ```text
//! header    8 bytes: "bplist" + a two-byte version, "00" for everything Finder writes
//! objects   one after another, each introduced by a marker byte
//! table     `num_objects` offsets, `offset_size` big-endian bytes each
//! trailer   last 32 bytes: 5 unused │ sort version │ offset_size │ ref_size
//!                          │ u64 num_objects │ u64 top_object │ u64 table_offset
//! ```
//!
//! A marker byte is a type in the high nibble and a count in the low one. A low
//! nibble of `0xF` means the real count follows as an integer object — which
//! every URL longer than fifteen characters hits, so it is the normal path
//! rather than an edge case.
//!
//! **Everything here is big-endian**, including the `0x6n` UTF-16 strings. That
//! is the opposite of every other format in this workspace.
//!
//! # Security
//!
//! An object reference is an index into the offset table and an offset table
//! entry is a position in the file; both come off the wire. A reference can
//! name its own container, so traversal is charged against
//! [`rclip_core::MAX_DEPTH`] the same way the bookmark reader's is.

use rclip_core::{Error, ErrorKind, Reader, Result, MAX_DEPTH};

use crate::text::Text;

/// The eight bytes a binary plist starts with.
pub const MAGIC: &[u8; 8] = b"bplist00";

/// Fixed size of the trailer at the end of the file.
const TRAILER_LEN: usize = 32;

/// Marker high nibbles.
mod marker {
    pub const SIMPLE: u8 = 0x0;
    pub const INT: u8 = 0x1;
    pub const ASCII: u8 = 0x5;
    pub const UTF16: u8 = 0x6;
    pub const UTF8: u8 = 0x7;
    pub const ARRAY: u8 = 0xA;
    pub const SET: u8 = 0xC;
    pub const DICT: u8 = 0xD;
}

/// A parsed binary plist header and offset table.
#[derive(Debug, Copy, Clone)]
pub struct BinaryPlist<'a> {
    buf: &'a [u8],
    /// Bytes per entry in the offset table.
    offset_size: usize,
    /// Bytes per object reference inside a container.
    ref_size: usize,
    num_objects: usize,
    top_object: usize,
    table_offset: usize,
}

/// As much of an object as this crate cares about.
#[derive(Debug, Copy, Clone)]
pub enum Object<'a> {
    /// A `0x5n`, `0x6n` or `0x7n` string.
    Str(Text<'a>),
    /// A `0xDn` dictionary: `count` key references followed by `count` value
    /// references, in the two slices.
    Dict {
        keys: &'a [u8],
        values: &'a [u8],
        count: usize,
    },
    /// Anything else — a number, a date, an array. Deliberately not decoded:
    /// a `.webloc` has no use for them and every type left undecoded is a type
    /// that cannot be a parser bug.
    Other,
}

impl<'a> BinaryPlist<'a> {
    /// `true` if the buffer starts with the binary plist signature.
    #[must_use]
    pub fn detect(buf: &[u8]) -> bool {
        buf.starts_with(MAGIC)
    }

    /// Validate the header, trailer and offset table.
    ///
    /// Everything the trailer claims is checked against the actual file length
    /// here, once, so that nothing downstream has to re-derive whether an
    /// object index is in range.
    pub fn parse(buf: &'a [u8]) -> Result<Self> {
        if !buf.starts_with(b"bplist") {
            return Err(Error::new(ErrorKind::BadMagic, 0));
        }
        if !buf.starts_with(MAGIC) {
            // "bplist15"/"bplist16" exist and are a different object encoding.
            // Nothing writes a .webloc in one, and pretending version 00 rules
            // apply would misread every object in the file.
            return Err(Error::new(ErrorKind::Unsupported, 6));
        }
        let trailer_at = buf
            .len()
            .checked_sub(TRAILER_LEN)
            .ok_or(Error::new(ErrorKind::UnexpectedEof, buf.len()))?;
        if trailer_at < MAGIC.len() {
            return Err(Error::new(ErrorKind::UnexpectedEof, buf.len()));
        }

        let r = Reader::new(buf);
        let mut t = Reader::new(r.slice_at(trailer_at, TRAILER_LEN)?);
        t.skip(6)?; // five unused bytes, then the sort version
        let offset_size = t.u8()? as usize;
        let ref_size = t.u8()? as usize;
        let num_objects = read_be_uint(t.take(8)?)? as usize;
        let top_object = read_be_uint(t.take(8)?)? as usize;
        let table_offset = read_be_uint(t.take(8)?)? as usize;

        // A zero size would make the offset table infinitely long; anything
        // over eight cannot be read into a u64 and never occurs.
        if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) {
            return Err(Error::new(ErrorKind::Malformed, trailer_at + 6));
        }
        if num_objects == 0 {
            return Err(Error::new(ErrorKind::Malformed, trailer_at + 8));
        }
        if top_object >= num_objects {
            return Err(Error::new(ErrorKind::BadOffset, trailer_at + 16));
        }
        if table_offset < MAGIC.len() || table_offset > trailer_at {
            return Err(Error::new(ErrorKind::BadOffset, trailer_at + 24));
        }
        // The offset table has to fit between where it starts and the trailer.
        // This is the check that keeps `num_objects` from being a promise the
        // file cannot keep — it is a u64 off the wire.
        let table_len = num_objects
            .checked_mul(offset_size)
            .ok_or(Error::new(ErrorKind::TooLarge, trailer_at + 8))?;
        let table_end = table_offset
            .checked_add(table_len)
            .ok_or(Error::new(ErrorKind::TooLarge, trailer_at + 8))?;
        if table_end > trailer_at {
            return Err(Error::new(ErrorKind::TooLarge, trailer_at + 8));
        }

        Ok(Self {
            buf,
            offset_size,
            ref_size,
            num_objects,
            top_object,
            table_offset,
        })
    }

    /// Index of the root object.
    #[must_use]
    pub const fn top_object(&self) -> usize {
        self.top_object
    }

    /// Number of objects the file declares.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.num_objects
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.num_objects == 0
    }

    /// Bytes per object reference, needed to walk a container's reference list.
    #[must_use]
    pub const fn ref_size(&self) -> usize {
        self.ref_size
    }

    /// Where object `index` starts.
    ///
    /// Objects are written before the offset table, so an offset at or past the
    /// table is out of bounds even though it is inside the file — without that
    /// check a crafted offset can make the reader parse the offset table, or
    /// the trailer, as an object.
    fn object_offset(&self, index: usize) -> Result<usize> {
        if index >= self.num_objects {
            return Err(Error::new(ErrorKind::BadOffset, self.table_offset));
        }
        // Cannot overflow: `parse` proved `num_objects * offset_size` fits and
        // lands before the trailer, and `index` is below `num_objects`.
        let at = self.table_offset + index * self.offset_size;
        let r = Reader::new(self.buf);
        let off = read_be_uint(r.slice_at(at, self.offset_size)?)? as usize;
        if off >= self.table_offset {
            return Err(Error::new(ErrorKind::BadOffset, at));
        }
        Ok(off)
    }

    /// Read the `index`-th object reference out of a container's reference
    /// list.
    pub fn reference(&self, refs: &'a [u8], index: usize) -> Result<usize> {
        let start = index
            .checked_mul(self.ref_size)
            .ok_or(Error::new(ErrorKind::TooLarge, self.table_offset))?;
        let bytes = refs
            .get(start..start + self.ref_size)
            .ok_or(Error::new(ErrorKind::BadOffset, self.table_offset))?;
        Ok(read_be_uint(bytes)? as usize)
    }

    /// A node budget sized to this payload, for [`BinaryPlist::object`].
    ///
    /// One visit per byte of input. A walk that resolves more objects than the
    /// file has bytes is re-treading shared references rather than reading new
    /// data, which is the only way a small file becomes an expensive one.
    #[must_use]
    pub const fn budget(&self) -> usize {
        self.buf.len() + 1
    }

    /// Decode object `index`.
    ///
    /// `depth` is how many containers were opened to get here: a dictionary
    /// whose value reference names the dictionary itself would otherwise
    /// recurse forever, so it costs a level per hop and stops at
    /// [`rclip_core::MAX_DEPTH`].
    ///
    /// `budget` is the second half, and it is not optional. **Depth alone is
    /// not enough**, and this format has the measurement to prove it: 223 bytes
    /// of nested dictionaries, nesting only nine levels so no depth limit ever
    /// fires, cost 40 million resolutions and 5.8 seconds to walk. Objects here
    /// are addressed by index, so one dictionary may name the same object many
    /// times and a graph that is tiny on disk can be enormous to traverse.
    ///
    /// The budget is `&mut` precisely so that siblings share it — a per-path
    /// counter would reset on every branch and catch nothing. Start it at
    /// [`BinaryPlist::budget`]. Exhausting it is [`ErrorKind::TooLarge`].
    ///
    /// `rclip-bookmark` carries the same guard for the same reason; the two
    /// formats are both index-addressed graphs and share the failure mode.
    pub fn object(&self, index: usize, depth: u32, budget: &mut usize) -> Result<Object<'a>> {
        let at = self.object_offset(index)?;
        if depth >= MAX_DEPTH {
            return Err(Error::new(ErrorKind::DepthLimit, at));
        }
        match budget.checked_sub(1) {
            Some(rest) => *budget = rest,
            None => return Err(Error::new(ErrorKind::TooLarge, at)),
        }

        let mut r = Reader::new(self.buf);
        r.seek(at)?;
        let m = r.u8()?;
        let kind = m >> 4;
        let low = usize::from(m & 0x0F);

        match kind {
            marker::ASCII | marker::UTF8 => {
                let count = self.count(&mut r, low, at)?;
                let bytes = r.take(count)?;
                // 0x5n is documented as ASCII, but treating it as UTF-8 costs
                // nothing and is right either way; what matters is that invalid
                // bytes are rejected rather than reinterpreted.
                let s = core::str::from_utf8(bytes)
                    .map_err(|_| Error::new(ErrorKind::InvalidUtf8, at))?;
                Ok(Object::Str(Text::Utf8(s)))
            }
            marker::UTF16 => {
                // The count is in UTF-16 code units, not bytes. Doubling it
                // before the bounds check is what keeps a count of 2^63 from
                // wrapping into a small byte length.
                let units = self.count(&mut r, low, at)?;
                let bytes = units
                    .checked_mul(2)
                    .ok_or(Error::new(ErrorKind::TooLarge, at))?;
                Ok(Object::Str(Text::Utf16Be(r.take(bytes)?)))
            }
            marker::DICT => {
                let count = self.count(&mut r, low, at)?;
                let total = count
                    .checked_mul(self.ref_size)
                    .ok_or(Error::new(ErrorKind::TooLarge, at))?;
                // Two reference lists of `count` entries each: keys then values.
                r.check_count(count, self.ref_size * 2)?;
                let keys = r.take(total)?;
                let values = r.take(total)?;
                Ok(Object::Dict {
                    keys,
                    values,
                    count,
                })
            }
            marker::SIMPLE | marker::INT | marker::ARRAY | marker::SET => Ok(Object::Other),
            _ => Ok(Object::Other),
        }
    }

    /// Resolve a marker's element count, following the `0xF` escape.
    ///
    /// A low nibble of `0xF` means the count did not fit in four bits and
    /// follows as an integer object. Every string longer than fifteen bytes
    /// takes this path, so it is the common case, not the corner.
    fn count(&self, r: &mut Reader<'a>, low: usize, at: usize) -> Result<usize> {
        if low != 0x0F {
            return Ok(low);
        }
        let m = r.u8()?;
        if m >> 4 != marker::INT {
            return Err(Error::new(ErrorKind::Malformed, r.pos() - 1));
        }
        let size = 1usize << (m & 0x0F);
        if size > 8 {
            // A 16-byte integer is legal as a *value* but never as a count;
            // nothing can hold 2^64 elements.
            return Err(Error::new(ErrorKind::TooLarge, r.pos() - 1));
        }
        let value = read_be_uint(r.take(size)?)?;
        usize::try_from(value).map_err(|_| Error::new(ErrorKind::TooLarge, at))
    }
}

/// Fold 1..=8 big-endian bytes into a `u64`.
fn read_be_uint(bytes: &[u8]) -> Result<u64> {
    if bytes.is_empty() || bytes.len() > 8 {
        return Err(Error::new(ErrorKind::Malformed, 0));
    }
    let mut out = 0u64;
    for &b in bytes {
        out = (out << 8) | u64::from(b);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bplist00` + a one-pair dict + the string "URL", with a hand-written
    /// trailer. The same shape as the self-referential corpus fixture, but with
    /// the value reference pointing at the string instead of at the dict.
    fn tiny() -> [u8; 53] {
        let mut b = [0u8; 53];
        b[..8].copy_from_slice(MAGIC);
        b[8..11].copy_from_slice(&[0xD1, 0x01, 0x02]); // dict, key ref 1, value ref 2
        b[11..15].copy_from_slice(&[0x53, b'U', b'R', b'L']);
        b[15..17].copy_from_slice(&[0x51, b'x']);
        b[17..20].copy_from_slice(&[8, 11, 15]); // offset table
        b[20..25].copy_from_slice(&[0; 5]);
        b[25] = 0; // sort version
        b[26] = 1; // offset size
        b[27] = 1; // ref size
        b[28..36].copy_from_slice(&3u64.to_be_bytes());
        b[36..44].copy_from_slice(&0u64.to_be_bytes());
        b[44..52].copy_from_slice(&17u64.to_be_bytes());
        b
    }

    #[test]
    fn tiny_plist_parses() {
        let bytes = tiny();
        let p = BinaryPlist::parse(&bytes[..52]).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p.top_object(), 0);
        let Object::Dict {
            keys,
            values,
            count,
        } = p.object(0, 0, &mut p.budget()).unwrap()
        else {
            panic!("root is a dictionary");
        };
        assert_eq!(count, 1);
        assert_eq!(p.reference(keys, 0).unwrap(), 1);
        assert_eq!(p.reference(values, 0).unwrap(), 2);
        let Object::Str(k) = p.object(1, 1, &mut p.budget()).unwrap() else {
            panic!("key is a string")
        };
        assert!(k.eq_str("URL"));
    }

    #[test]
    fn offset_into_the_table_is_rejected() {
        let mut bytes = tiny();
        bytes[17] = 18; // object 0 now starts inside the offset table
        let p = BinaryPlist::parse(&bytes[..52]).unwrap();
        assert_eq!(
            p.object(0, 0, &mut p.budget()).unwrap_err().kind,
            ErrorKind::BadOffset
        );
    }

    #[test]
    fn out_of_range_object_index_is_rejected() {
        let bytes = tiny();
        let p = BinaryPlist::parse(&bytes[..52]).unwrap();
        assert_eq!(
            p.object(99, 0, &mut p.budget()).unwrap_err().kind,
            ErrorKind::BadOffset
        );
    }

    #[test]
    fn absurd_object_count_is_rejected() {
        let mut bytes = tiny();
        bytes[28..36].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(matches!(
            BinaryPlist::parse(&bytes[..52]).unwrap_err().kind,
            ErrorKind::TooLarge | ErrorKind::BadOffset
        ));
    }

    #[test]
    fn zero_offset_size_is_rejected() {
        let mut bytes = tiny();
        bytes[26] = 0;
        assert_eq!(
            BinaryPlist::parse(&bytes[..52]).unwrap_err().kind,
            ErrorKind::Malformed
        );
    }

    #[test]
    fn depth_limit_is_enforced() {
        let bytes = tiny();
        let p = BinaryPlist::parse(&bytes[..52]).unwrap();
        assert_eq!(
            p.object(0, MAX_DEPTH, &mut p.budget()).unwrap_err().kind,
            ErrorKind::DepthLimit
        );
    }
}

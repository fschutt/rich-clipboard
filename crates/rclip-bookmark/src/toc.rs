//! Tables of contents and their entries.
//!
//! A bookmark's payload starts with a `u32` offset to the first TOC; each TOC
//! ends with an offset to the next, or zero. Every entry maps a key to the
//! offset of a data record.

use rclip_core::{Error, ErrorKind, Reader, Result};

use crate::value::Value;
use crate::Bookmark;

/// Sentinel at offset 4 of every TOC. A mismatch means the offset that led here
/// was wrong, which is a much more useful thing to report than whatever the
/// bytes at that address happen to decode to.
pub const TOC_MAGIC: u32 = 0xFFFF_FFFE;

/// Bytes of TOC header before the first entry: size, magic, id, next, count.
const TOC_HEADER_LEN: usize = 20;
/// Bytes per TOC entry: key, data offset, reserved.
const TOC_ENTRY_LEN: usize = 12;

/// One table of contents.
#[derive(Debug, Copy, Clone)]
pub struct Toc<'a> {
    bm: Bookmark<'a>,
    id: u32,
    next: u32,
    at: usize,
    entries: &'a [u8],
}

impl<'a> Toc<'a> {
    /// The TOC's identifier. Key [`crate::key::VOLUME_BOOKMARK`] refers to an
    /// embedded bookmark by this number rather than by offset.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// Payload-relative offset of the next TOC, or zero at the end of the
    /// chain. Exposed mostly so a dumping tool can show the chain.
    #[must_use]
    pub const fn next_offset(&self) -> u32 {
        self.next
    }

    /// Absolute offset of this TOC in the caller's buffer.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.at
    }

    /// Number of entries.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len() / TOC_ENTRY_LEN
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Entry `index`, without resolving its key or value.
    pub fn get(&self, index: usize) -> Result<Entry<'a>> {
        let start = index
            .checked_mul(TOC_ENTRY_LEN)
            .ok_or(Error::new(ErrorKind::TooLarge, self.at))?;
        let raw = self
            .entries
            .get(start..start + TOC_ENTRY_LEN)
            .ok_or(Error::new(ErrorKind::BadOffset, self.at))?;
        let mut r = Reader::new(raw);
        let raw_key = r.u32_le()?;
        let value_offset = r.u32_le()?;
        // The third word is documented as reserved-zero by mac_alias and as
        // "possibly flags" by Mother's Ruin. Nothing observed in the wild uses
        // it, so it is read and dropped rather than validated — rejecting a
        // bookmark over a field nobody can explain would be a bad trade.
        let _reserved = r.u32_le()?;
        Ok(Entry {
            bm: self.bm,
            raw_key,
            value_offset,
            at: self.at + start,
        })
    }

    #[must_use]
    pub const fn iter(&self) -> EntryIter<'a> {
        EntryIter {
            toc: *self,
            index: 0,
        }
    }

    /// Parse the TOC at payload-relative offset `rel`.
    pub(crate) fn parse(bm: &Bookmark<'a>, rel: u32) -> Result<Self> {
        let at = bm.abs(rel)?;
        let mut r = Reader::new(bm.data());
        r.seek(at)?;

        // The size field counts everything after itself and the magic, so the
        // whole TOC is eight bytes longer than it says.
        let size = r.u32_le()? as usize;
        let magic = r.u32_le()?;
        if magic != TOC_MAGIC {
            return Err(Error::new(ErrorKind::BadMagic, at + 4));
        }
        let id = r.u32_le()?;
        let next = r.u32_le()?;
        let count = r.u32_le()? as usize;

        let total = size
            .checked_add(8)
            .ok_or(Error::new(ErrorKind::TooLarge, at))?;
        let end = at
            .checked_add(total)
            .ok_or(Error::new(ErrorKind::TooLarge, at))?;
        if end > bm.data().len() {
            return Err(Error::new(ErrorKind::BadLength, at));
        }
        // The entries have to fit inside the TOC's own declared extent. This is
        // stricter than mac_alias, which compares the entry bytes against the
        // TOC size while forgetting the 20-byte header; a count that overruns
        // by one entry slips past that check and reads a neighbouring record's
        // bytes as an entry.
        let entry_bytes = count
            .checked_mul(TOC_ENTRY_LEN)
            .ok_or(Error::new(ErrorKind::TooLarge, at))?;
        if TOC_HEADER_LEN + entry_bytes > total {
            return Err(Error::new(ErrorKind::BadLength, at));
        }
        // …and inside the buffer, which is what keeps `count` from being an
        // unbacked promise even if the TOC size field is complicit.
        r.check_count(count, TOC_ENTRY_LEN)?;
        let entries = r.take(entry_bytes)?;

        Ok(Self {
            bm: *bm,
            id,
            next,
            at,
            entries,
        })
    }
}

impl<'a> IntoIterator for Toc<'a> {
    type Item = Result<Entry<'a>>;
    type IntoIter = EntryIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Iterator over the entries of a [`Toc`].
#[derive(Debug, Clone)]
pub struct EntryIter<'a> {
    toc: Toc<'a>,
    index: usize,
}

impl<'a> Iterator for EntryIter<'a> {
    type Item = Result<Entry<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.toc.len() {
            return None;
        }
        let out = self.toc.get(self.index);
        self.index += 1;
        Some(out)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.toc.len().saturating_sub(self.index);
        (n, Some(n))
    }
}

impl ExactSizeIterator for EntryIter<'_> {}

/// A TOC entry's key: a number from [`crate::key`], or a name.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum EntryKey<'a> {
    /// One of the numbers in [`crate::key`], or an unidentified one.
    Numeric(u32),
    /// A key named by a string record, flagged by bit 31 of the key word.
    Named(&'a str),
}

impl EntryKey<'_> {
    /// The numeric key, if this is one.
    #[must_use]
    pub const fn as_numeric(&self) -> Option<u32> {
        match self {
            Self::Numeric(k) => Some(*k),
            Self::Named(_) => None,
        }
    }

    /// The key's name — either the string it carries, or the table name for a
    /// recognised number.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Numeric(k) => crate::key::name(*k),
            Self::Named(s) => Some(s),
        }
    }
}

/// One key-to-record mapping inside a [`Toc`].
#[derive(Debug, Copy, Clone)]
pub struct Entry<'a> {
    bm: Bookmark<'a>,
    raw_key: u32,
    value_offset: u32,
    at: usize,
}

impl<'a> Entry<'a> {
    /// The key word exactly as stored, bit 31 and all.
    #[must_use]
    pub const fn raw_key(&self) -> u32 {
        self.raw_key
    }

    /// Payload-relative offset of the entry's value record.
    #[must_use]
    pub const fn value_offset(&self) -> u32 {
        self.value_offset
    }

    /// Absolute offset of the entry itself in the caller's buffer.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.at
    }

    /// `true` when the key is a string reference rather than a number.
    #[must_use]
    pub const fn has_named_key(&self) -> bool {
        self.raw_key & crate::key::STRING_KEY_FLAG != 0
    }

    /// The key, following the bit-31 indirection when it is set.
    pub fn key(&self) -> Result<EntryKey<'a>> {
        if !self.has_named_key() {
            return Ok(EntryKey::Numeric(self.raw_key));
        }
        // Depth 0: a key's string record is a leaf, and if it is not, the
        // resolver rejects it below rather than descending.
        let off = self.raw_key & crate::key::STRING_KEY_MASK;
        match self.bm.value_at(off, 0)? {
            Value::Str(s) => Ok(EntryKey::Named(s)),
            // A key that points at, say, an array would let a hostile bookmark
            // smuggle an object graph in through the key side of the table.
            _ => Err(Error::new(ErrorKind::Malformed, self.at)),
        }
    }

    /// The entry's value record.
    pub fn value(&self) -> Result<Value<'a>> {
        self.bm.value_at(self.value_offset, 0)
    }
}

/// Iterator over the TOC chain of a bookmark.
#[derive(Debug, Clone)]
pub struct TocIter<'a> {
    bm: Bookmark<'a>,
    next: u32,
    budget: usize,
    done: bool,
}

impl<'a> TocIter<'a> {
    pub(crate) fn new(bm: &Bookmark<'a>, first: u32) -> Self {
        // A TOC cannot be shorter than its header, so a payload of N bytes can
        // hold at most N/20 of them. Anything longer means the chain revisits a
        // TOC — and `next` pointing back at the current TOC is a plain infinite
        // loop, which is exactly what mac_alias does on such a file.
        let budget = bm.data().len() / TOC_HEADER_LEN + 1;
        Self {
            bm: *bm,
            next: first,
            budget,
            done: false,
        }
    }
}

impl<'a> Iterator for TocIter<'a> {
    type Item = Result<Toc<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.next == 0 {
            return None;
        }
        if self.budget == 0 {
            self.done = true;
            return Some(Err(Error::new(ErrorKind::Malformed, self.bm.header_size())));
        }
        self.budget -= 1;

        match Toc::parse(&self.bm, self.next) {
            Ok(toc) => {
                self.next = toc.next_offset();
                Some(Ok(toc))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

//! `CIDA` — the payload behind the `CFSTR_SHELLIDLIST` ("Shell IDList Array")
//! clipboard format.
//!
//! Unlike the contents of a PIDL, this outer wrapper *is* documented:
//!
//! ```c
//! typedef struct _IDA {
//!     UINT cidl;          // number of child PIDLs
//!     UINT aoffset[1];    // cidl + 1 offsets, from the start of the CIDA
//! } CIDA;
//! ```
//!
//! `aoffset[0]` locates the fully qualified PIDL of the parent folder;
//! `aoffset[1]` through `aoffset[cidl]` locate PIDLs *relative to that parent*.
//! A consumer that wants an absolute PIDL concatenates the two — which this
//! crate does not do for you, because concatenation needs an allocator and
//! because most callers only want the leaf name.
//!
//! See <https://learn.microsoft.com/en-us/windows/win32/shell/clipboard>.
//!
//! # What is hostile here
//!
//! `cidl` and every entry of `aoffset` come off the clipboard from another
//! process. `cidl` sizes a loop, and each offset indexes the buffer. Both are
//! validated before use: `cidl + 1` against the bytes actually present, and each
//! offset through [`rclip_core::Reader::tail_at`].

use rclip_core::{Error, ErrorKind, Reader, Result};

use crate::item::ItemIdList;

/// A parsed `CIDA` header. Borrows the whole payload, because every offset in
/// it is relative to the start of the structure.
#[derive(Debug, Clone)]
pub struct Cida<'a> {
    buf: &'a [u8],
    count: usize,
}

impl<'a> Cida<'a> {
    /// Validate the header and the offset table. Does not touch the PIDLs
    /// themselves; those are parsed lazily and individually so that one broken
    /// child cannot cost you the other nine.
    pub fn parse(buf: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(buf);
        let cidl = r.u32_le()?;

        // usize is 16 bits on some embedded targets, and `cidl` is 32; converting
        // through try_from keeps the failure an error instead of a truncation
        // that would make the offset table look shorter than it is.
        let count = usize::try_from(cidl).map_err(|_| Error::new(ErrorKind::TooLarge, 0))?;

        // cidl + 1 offsets: the parent plus one per child. Checked against the
        // bytes that are actually here before it is used as a loop bound.
        let entries = count
            .checked_add(1)
            .ok_or_else(|| r.err(ErrorKind::TooLarge))?;
        r.check_count(entries, 4)?;

        Ok(Self { buf, count })
    }

    /// Number of child items. The offset table holds one more entry than this.
    #[must_use]
    pub const fn child_count(&self) -> usize {
        self.count
    }

    /// The whole payload, for callers that want to re-serialize it untouched.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.buf
    }

    /// `aoffset[index]`, where index 0 is the parent. Bounds-checked against
    /// `cidl`, and the value is checked to land inside the buffer.
    pub fn offset(&self, index: usize) -> Result<usize> {
        if index > self.count {
            return Err(Error::new(ErrorKind::BadOffset, 0));
        }
        // 4 for cidl, then four bytes per entry. Cannot overflow: `parse`
        // already proved this many entries fit in a slice that exists.
        let at = 4 + index * 4;
        let r = Reader::new(self.buf);
        let raw = r.peek_u32_le_at(at)?;
        let off = usize::try_from(raw).map_err(|_| Error::new(ErrorKind::TooLarge, at))?;
        // Reject here rather than at first use, so a bad offset is reported
        // against the table entry that carried it.
        if off >= self.buf.len() {
            return Err(Error::new(ErrorKind::BadOffset, at));
        }
        Ok(off)
    }

    /// The parent folder's fully qualified PIDL, `aoffset[0]`.
    pub fn parent(&self) -> Result<ItemIdList<'a>> {
        self.list_at(0)
    }

    /// The `index`-th child PIDL, relative to [`Cida::parent`].
    pub fn child(&self, index: usize) -> Result<ItemIdList<'a>> {
        let entry = index
            .checked_add(1)
            .ok_or(Error::new(ErrorKind::TooLarge, 0))?;
        if entry > self.count {
            return Err(Error::new(ErrorKind::BadOffset, 0));
        }
        self.list_at(entry)
    }

    /// Every child PIDL in order.
    ///
    /// Yields one `Result` per child; a child with an out-of-range offset yields
    /// `Err` without stopping the iteration, since the remaining children are
    /// still readable and a paste of nine out of ten files beats a paste of
    /// none.
    #[must_use]
    pub fn children(&self) -> CidaChildren<'a> {
        CidaChildren {
            cida: self.clone(),
            next: 0,
        }
    }

    fn list_at(&self, entry: usize) -> Result<ItemIdList<'a>> {
        let off = self.offset(entry)?;
        let tail = Reader::new(self.buf).tail_at(off)?;
        Ok(ItemIdList::with_base(tail, off))
    }
}

/// Iterator over the child PIDLs of a [`Cida`].
#[derive(Debug, Clone)]
pub struct CidaChildren<'a> {
    cida: Cida<'a>,
    next: usize,
}

impl<'a> Iterator for CidaChildren<'a> {
    type Item = Result<ItemIdList<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.cida.count {
            return None;
        }
        let i = self.next;
        self.next += 1;
        Some(self.cida.child(i))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.cida.count - self.next;
        (n, Some(n))
    }
}

impl ExactSizeIterator for CidaChildren<'_> {}
impl core::iter::FusedIterator for CidaChildren<'_> {}

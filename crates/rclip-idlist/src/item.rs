//! The `SHITEMID` list walk — the one piece of this crate that has to be right.
//!
//! An `ITEMIDLIST` is a sequence of variable-length records terminated by a
//! zero `u16`:
//!
//! ```text
//! SHITEMID { u16 cb; u8 abID[cb - 2]; }   // cb counts itself
//! IDLIST   = *SHITEMID  u16(0)
//! ```
//!
//! `cb` is the whole trap. It is a length field read from an attacker-controlled
//! buffer, it counts its own two bytes, and it is also the amount the walk
//! advances by. A parser that trusts it and writes `pos += cb` hangs forever the
//! first time someone hands it a `cb` of zero that is not meant as a terminator,
//! and misaligns itself on a `cb` of one. Both cases are handled explicitly
//! below and both are covered by fixtures.

use rclip_core::{Error, ErrorKind, Result};

use crate::shell_item::ShellItem;

/// Smallest well-formed `cb`: the two bytes of the size field itself, i.e. an
/// item with an empty `abID`.
pub const MIN_ITEM_SIZE: usize = 2;

/// One raw `SHITEMID`, located within the list it came from.
///
/// Parsing stops here on purpose. The list walk is structural and can fail; the
/// *contents* of an item are a reverse-engineered guess and must never fail, so
/// interpreting them is a separate, infallible step ([`ItemId::parse`]).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ItemId<'a> {
    /// Byte offset of this item's `cb` field from the start of the list buffer.
    ///
    /// Carried so a caller that finds something odd deep inside an item can
    /// point at a byte in the original capture.
    pub offset: usize,
    /// `abID`: the item body, exactly `cb - 2` bytes. May be empty.
    pub data: &'a [u8],
}

impl<'a> ItemId<'a> {
    /// The item's on-the-wire size, including the two bytes of `cb`.
    #[must_use]
    pub const fn cb(&self) -> usize {
        self.data.len() + MIN_ITEM_SIZE
    }

    /// The class type indicator: the first byte of `abID`.
    ///
    /// Returns `None` for an empty item (`cb == 2`), which is legal and appears
    /// as a "my desktop" placeholder at the head of some lists.
    #[must_use]
    pub fn class(&self) -> Option<u8> {
        self.data.first().copied()
    }

    /// Interpret the body. Never fails — see [`ShellItem::Unknown`].
    #[must_use]
    pub fn parse(&self) -> ShellItem<'a> {
        ShellItem::parse(self.data)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Walk {
    /// More items may follow.
    Running,
    /// The list ended, cleanly.
    Ended,
    /// The list is malformed; the error was already yielded.
    Failed,
}

/// Iterator over the `SHITEMID`s of an `ITEMIDLIST`.
///
/// Yields `Err` at most once: the first structural problem stops the walk, so a
/// caller can `for item in list { ... }` without risking an endless stream of
/// errors from a buffer full of garbage.
///
/// # Where the list ends
///
/// Iteration ends cleanly on an explicit `u16` terminator, or when the buffer
/// runs out on an exact item boundary. It ends with [`ErrorKind::UnexpectedEof`]
/// when a *partial* item is left over, because that is the case where data was
/// actually lost. Callers that know the declared extent of the list — a `.lnk`
/// `LinkTargetIDList` knows `IDListSize` — can additionally require
/// [`ItemIdList::is_terminated`].
#[derive(Debug, Clone)]
pub struct ItemIdList<'a> {
    buf: &'a [u8],
    pos: usize,
    base: usize,
    state: Walk,
    terminated: bool,
}

impl<'a> ItemIdList<'a> {
    /// Walk a list that starts at the beginning of `buf`.
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self::with_base(buf, 0)
    }

    /// Same, but reporting errors at `base + offset`.
    ///
    /// `CIDA` and `.lnk` both embed lists at an offset inside a larger payload;
    /// without this the offset in an [`Error`] would be relative to a slice the
    /// caller no longer has, which makes a corpus mismatch much harder to chase.
    #[must_use]
    pub const fn with_base(buf: &'a [u8], base: usize) -> Self {
        Self { buf, pos: 0, base, state: Walk::Running, terminated: false }
    }

    /// Bytes consumed so far, including the terminator once it is reached.
    ///
    /// Only meaningful after the iterator has been driven to completion.
    #[must_use]
    pub const fn bytes_consumed(&self) -> usize {
        self.pos
    }

    /// `true` if the walk stopped on an explicit `u16` zero terminator rather
    /// than on the end of the buffer.
    #[must_use]
    pub const fn is_terminated(&self) -> bool {
        self.terminated
    }

    /// `true` if the walk stopped because the list was malformed.
    #[must_use]
    pub const fn failed(&self) -> bool {
        matches!(self.state, Walk::Failed)
    }

    /// Count the items, returning the structural error if there is one.
    ///
    /// Safe to call on hostile input: every step of the walk advances by at
    /// least [`MIN_ITEM_SIZE`], so this terminates in at most `buf.len() / 2`
    /// iterations.
    pub fn try_len(&self) -> Result<usize> {
        let mut n = 0usize;
        for item in self.clone() {
            item?;
            n += 1;
        }
        Ok(n)
    }
}

impl<'a> Iterator for ItemIdList<'a> {
    type Item = Result<ItemId<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.state != Walk::Running {
            return None;
        }
        let here = self.base + self.pos;
        let rest = self.buf.get(self.pos..)?;

        // Running out exactly on an item boundary is a normal end of list: a
        // PIDL sliced out of a bigger structure by a length field often carries
        // no terminator of its own.
        if rest.is_empty() {
            self.state = Walk::Ended;
            return None;
        }
        // One stray byte cannot be a `cb`, and silently dropping it would hide a
        // truncation, so this is an error rather than an end.
        if rest.len() < MIN_ITEM_SIZE {
            self.state = Walk::Failed;
            return Some(Err(Error::new(ErrorKind::UnexpectedEof, here)));
        }

        let cb = usize::from(u16::from_le_bytes([rest[0], rest[1]]));

        // TerminalID. Must be tested before the `cb < MIN_ITEM_SIZE` check
        // below, because zero is the one value that means "stop" rather than
        // "malformed".
        if cb == 0 {
            self.pos += MIN_ITEM_SIZE;
            self.terminated = true;
            self.state = Walk::Ended;
            return None;
        }
        // cb == 1: a size that does not even cover its own size field. Left
        // unchecked this is either an infinite loop (if the walk clamps the
        // advance to zero) or a permanently misaligned parse. Reject the list.
        if cb < MIN_ITEM_SIZE {
            self.state = Walk::Failed;
            return Some(Err(Error::new(ErrorKind::BadLength, here)));
        }
        if cb > rest.len() {
            self.state = Walk::Failed;
            return Some(Err(Error::new(ErrorKind::UnexpectedEof, here)));
        }

        let item = ItemId { offset: self.pos, data: &rest[MIN_ITEM_SIZE..cb] };
        self.pos += cb;
        Some(Ok(item))
    }
}

impl core::iter::FusedIterator for ItemIdList<'_> {}

impl<'a> From<&'a [u8]> for ItemIdList<'a> {
    fn from(buf: &'a [u8]) -> Self {
        Self::new(buf)
    }
}

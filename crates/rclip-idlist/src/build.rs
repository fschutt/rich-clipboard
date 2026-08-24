//! Assembling an `ITEMIDLIST`. Requires `alloc`.
//!
//! Deliberately low-level. Writing a *file entry* PIDL that Explorer will accept
//! is not a matter of getting the byte layout right — the shell binds a PIDL by
//! asking the namespace extension that owns it to parse the bytes back, so a
//! hand-forged file entry either resolves to something unintended or does not
//! resolve at all. The safe pattern is to copy PIDL bytes you were *given*
//! (from a `CFSTR_SHELLIDLIST` you received, or from `SHGetIDListFromObject`)
//! and re-emit them, which is what [`ItemIdListBuilder::push_raw`] is for.
//!
//! The one item this builder synthesises is a root folder item, because that one
//! is fully determined: a class byte, a sort index, and a GUID.

extern crate alloc;

use alloc::vec::Vec;

use crate::{guid::Guid, item::MIN_ITEM_SIZE, shell_item::CLASS_ROOT_FOLDER};

/// Builds the bytes of an `ITEMIDLIST`.
///
/// The terminating `u16` zero is written by [`ItemIdListBuilder::finish`], not
/// as you go, so a half-built list can never be mistaken for a complete one.
#[derive(Debug, Clone, Default)]
pub struct ItemIdListBuilder {
    buf: Vec<u8>,
}

impl ItemIdListBuilder {
    #[must_use]
    pub const fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Append an item whose body (`abID`) is `data`, prefixing the `cb` field.
    ///
    /// Returns `false` and appends nothing if the item would not fit in a `u16`
    /// `cb`. Silently truncating here would produce a list that parses as
    /// something other than what was asked for.
    pub fn push_raw(&mut self, data: &[u8]) -> bool {
        let Ok(cb) = u16::try_from(data.len() + MIN_ITEM_SIZE) else {
            return false;
        };
        self.buf.extend_from_slice(&cb.to_le_bytes());
        self.buf.extend_from_slice(data);
        true
    }

    /// Append a root folder item: class `0x1F`, a sort index, and a shell folder
    /// GUID. 20 bytes on the wire.
    pub fn push_root_folder(&mut self, sort_index: u8, guid: &Guid) {
        let mut body = [0u8; 18];
        body[0] = CLASS_ROOT_FOLDER;
        body[1] = sort_index;
        body[2..].copy_from_slice(guid.as_bytes());
        // 20 bytes always fits a u16, so the return value cannot be false here.
        let _ = self.push_raw(&body);
    }

    /// Bytes written so far, excluding the terminator.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Append the `u16` terminator and hand back the list.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf
    }
}

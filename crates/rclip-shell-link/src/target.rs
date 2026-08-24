//! MS-SHLLINK 2.2 — `LinkTargetIDList`.
//!
//! A `u16` size followed by an `IDList`, which is where this crate hands off to
//! [`rclip_idlist`]. The same structure appears again, without the size prefix,
//! inside the `VistaAndAboveIDListDataBlock`.

use rclip_core::{Reader, Result};
use rclip_idlist::ItemIdList;

/// MS-SHLLINK 2.2 — the shell namespace path to the link's target.
///
/// This is the *authoritative* target of a shell link. Everything else in the
/// file — `LinkInfo`, `RELATIVE_PATH`, the environment variable block — is a
/// fallback for when the shell cannot bind this IDList.
#[derive(Debug, Clone)]
pub struct LinkTargetIdList<'a> {
    /// `IDListSize`. Counts the `IDList` field, **including** its two-byte
    /// terminator — confirmed by MS-SHLLINK 3.1, where `IDListSize = 0xBD` is
    /// 187 bytes of items plus the 2-byte `TerminalID`. So the structure
    /// occupies `2 + id_list_size` bytes.
    pub id_list_size: u16,
    bytes: &'a [u8],
}

impl<'a> LinkTargetIdList<'a> {
    /// Read from the cursor, advancing it past the whole structure.
    pub(crate) fn parse(r: &mut Reader<'a>) -> Result<Self> {
        let id_list_size = r.u16_le()?;
        // take() bounds the inner list to its declared extent, so a malformed
        // IDList cannot walk into the LinkInfo that follows it.
        let bytes = r.take(usize::from(id_list_size))?;
        Ok(Self {
            id_list_size,
            bytes,
        })
    }

    /// The `IDList` bytes, terminator included.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Walk the shell items.
    ///
    /// Structural errors surface per-item; see [`ItemIdList`]. Nothing here
    /// resolves the IDList against a live shell — see the crate docs.
    #[must_use]
    pub const fn items(&self) -> ItemIdList<'a> {
        ItemIdList::new(self.bytes)
    }

    /// Total on-the-wire size, including the `IDListSize` field itself.
    #[must_use]
    pub const fn wire_size(&self) -> usize {
        2 + self.id_list_size as usize
    }
}

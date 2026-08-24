//! Windows shell `ITEMIDLIST` (PIDL) and `CIDA` parsing.
//!
//! A PIDL is how the Windows shell names a thing. Not every thing in the shell
//! namespace is a file — the Recycle Bin, a camera, a mail message, a search
//! result and a zip file's interior all have PIDLs and none of them have paths —
//! so when Explorer puts a selection on the clipboard as `CFSTR_SHELLIDLIST`
//! ("Shell IDList Array"), a PIDL is the only thing it can offer that describes
//! all of them.
//!
//! ```text
//! CIDA      { u32 cidl; u32 aoffset[cidl + 1]; }   // documented by Microsoft
//! IDLIST    = *SHITEMID u16(0)                     // documented
//! SHITEMID  { u16 cb; u8 abID[cb - 2]; }           // cb documented, abID not
//! ```
//!
//! # What is and is not documented
//!
//! The outer wrapper is public: [`Cida`] follows the `CIDA` struct in
//! `shlobj_core.h`, and the `SHITEMID` chain-of-lengths is in `shtypes.h`.
//! The **contents** of `abID` are not documented by Microsoft, by design — each
//! namespace extension owns the bytes of its own item IDs and may change them
//! between releases. What this crate knows about them comes from the ShellBags
//! forensics community, whose reference description is libyal's
//! [Windows Shell Item format](https://github.com/libyal/libfwsi/tree/main/documentation).
//!
//! That has a direct consequence for the API: **item parsing never fails.**
//! Anything unrecognised comes back as [`ShellItem::Unknown`] with its class
//! byte and its raw bytes intact. A PIDL that came from a shell extension
//! nobody has reverse-engineered must not be able to break a paste — the
//! caller almost always just wants "a display name if you can manage one", and
//! the items it *can* read are still worth having.
//!
//! Structural parsing, in contrast, is strict: see [`ItemIdList`].
//!
//! # The `cb` trap
//!
//! `cb` counts its own two bytes and is also the walk's stride. A `cb` below two
//! therefore advances the cursor by less than the field it just read, and a
//! parser that does not check for it either spins forever or reparses the same
//! bytes at a sliding offset. [`ItemIdList`] treats `cb == 0` as the list
//! terminator it is defined to be and rejects `cb == 1` with
//! [`ErrorKind::BadLength`](rclip_core::ErrorKind::BadLength). There is a
//! fixture for each.
//!
//! # Security
//!
//! This crate reads bytes and returns data. It never resolves a PIDL, never
//! binds to a namespace extension, never touches the filesystem, and never
//! consults the registry. A PIDL is a name, and turning a name into an object is
//! the operation that has historically been dangerous — that step belongs to the
//! caller, on a platform that has a shell, with whatever policy it wants to
//! apply.
//!
//! # Example
//!
//! ```
//! use rclip_idlist::{ItemIdList, ShellItem};
//!
//! // A one-item list: a root folder item for "My Computer", then the terminator.
//! let bytes = [
//!     0x14, 0x00, // cb = 20
//!     0x1F, 0x00, // class 0x1F (root folder), sort index 0
//!     0xE0, 0x4F, 0xD0, 0x20, 0xEA, 0x3A, 0x69, 0x10, // GUID
//!     0xA2, 0xD8, 0x08, 0x00, 0x2B, 0x30, 0x30, 0x9D,
//!     0x00, 0x00, // terminator
//! ];
//!
//! let mut list = ItemIdList::new(&bytes);
//! let item = list.next().unwrap().unwrap();
//! match item.parse() {
//!     ShellItem::RootFolder(root) => {
//!         assert_eq!(root.guid.well_known_name(), Some("My Computer"));
//!     }
//!     other => panic!("expected a root folder, got {other:?}"),
//! }
//! assert!(list.next().is_none());
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

#[cfg(feature = "std")]
extern crate std;

pub mod cida;
pub mod dostime;
pub mod guid;
pub mod item;
pub mod shell_item;
pub mod signature;
pub mod string;

#[cfg(feature = "alloc")]
pub mod build;

pub use cida::{Cida, CidaChildren};
pub use dostime::DosDateTime;
pub use guid::{Guid, GuidStr};
pub use item::{ItemId, ItemIdList, MIN_ITEM_SIZE};
/// Re-exported so a caller can name a code page without adding
/// `rclip-codepage` to its own manifest.
#[cfg(feature = "codepage")]
pub use rclip_codepage::Encoding;
pub use shell_item::{
    ExtensionBlock, FileEntry, FtpData, NetworkLocation, RootFolder, ShellItem, Uri, Volume,
};
pub use signature::{
    CompressedFolder, CompressedFolderVariant, ControlPanelItem, DelegateFolder, MtpFileEntry,
    MtpVolume, UsersPropertyView,
};
#[cfg(feature = "codepage")]
pub use string::ShellCharsWith;
pub use string::{ShellChars, ShellStr};

#[cfg(feature = "alloc")]
pub use build::ItemIdListBuilder;

#[cfg(feature = "alloc")]
extern crate alloc;

/// Join the display names of a list's items with `separator`.
///
/// Best effort by construction: items that have no display name — a control
/// panel applet, a namespace extension nobody has reverse-engineered — are
/// skipped rather than rendered as a placeholder, because a breadcrumb with a
/// gap is more honest than one with a fabricated segment. This is a *label*,
/// never a path to open.
#[cfg(feature = "alloc")]
#[must_use]
pub fn display_path(list: ItemIdList<'_>, separator: &str) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    for item in list {
        let Ok(item) = item else { break };
        let Some(name) = item.parse().display_name() else {
            continue;
        };
        if !out.is_empty() {
            out.push_str(separator);
        }
        out.push_str(&name.to_string_lossy());
    }
    out
}

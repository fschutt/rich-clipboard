//! MS-SHLLINK 2.4 — `StringData`.
//!
//! Five optional counted strings in a fixed order, each gated by a `LinkFlags`
//! bit. Two things about them are easy to get wrong and both are load bearing:
//!
//! 1. **`CountCharacters` counts characters, not bytes.** With `IsUnicode` set
//!    the field is `count * 2` bytes long. A parser that treats the count as a
//!    byte length reads half of each Unicode string and then desynchronises for
//!    every field that follows.
//! 2. **The multiply must happen after widening.** `count * 2` computed in
//!    `u16` wraps for any count at or above 32768 — silently, in release — and
//!    yields a short string plus a corrupted parse of everything after it. Two
//!    published crates get this wrong. `count` is widened to `usize` here
//!    before it is doubled.
//!
//! The strings are **not** NUL-terminated and the count excludes any terminator.

use rclip_core::{ErrorKind, Reader, Result};
use rclip_idlist::ShellStr;

use crate::header::LinkFlags;

/// The five `StringData` fields, in wire order.
///
/// Every one is optional and absent means the corresponding `LinkFlags` bit was
/// clear — not that the string was empty. An empty string with the flag set is a
/// different thing, and is represented as `Some` with a zero-length value.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct StringData<'a> {
    /// `NAME_STRING`: the shortcut's description, shown to the user.
    pub name: Option<ShellStr<'a>>,
    /// `RELATIVE_PATH`: the target's location relative to the `.lnk` file
    /// itself, e.g. `.\a.txt`. Used when the IDList will not bind.
    pub relative_path: Option<ShellStr<'a>>,
    /// `WORKING_DIR`: the directory to activate the target in.
    pub working_dir: Option<ShellStr<'a>>,
    /// `COMMAND_LINE_ARGUMENTS`: arguments passed on activation.
    ///
    /// This is the field that has historically been abused, because it is
    /// attacker-controlled text that a shell hands to a process. This crate
    /// returns it and does nothing else with it.
    pub arguments: Option<ShellStr<'a>>,
    /// `ICON_LOCATION`: where to load the display icon from.
    pub icon_location: Option<ShellStr<'a>>,
}

impl<'a> StringData<'a> {
    /// Read all five fields from the cursor, in the order the spec fixes.
    pub(crate) fn parse(r: &mut Reader<'a>, flags: LinkFlags) -> Result<Self> {
        let unicode = flags.is_unicode();
        let mut read = |bit: LinkFlags| -> Result<Option<ShellStr<'a>>> {
            if flags.contains(bit) {
                read_counted(r, unicode).map(Some)
            } else {
                Ok(None)
            }
        };
        Ok(Self {
            name: read(LinkFlags::HAS_NAME)?,
            relative_path: read(LinkFlags::HAS_RELATIVE_PATH)?,
            working_dir: read(LinkFlags::HAS_WORKING_DIR)?,
            arguments: read(LinkFlags::HAS_ARGUMENTS)?,
            icon_location: read(LinkFlags::HAS_ICON_LOCATION)?,
        })
    }

    /// `true` if none of the five fields is present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.relative_path.is_none()
            && self.working_dir.is_none()
            && self.arguments.is_none()
            && self.icon_location.is_none()
    }
}

/// One `CountCharacters` + `String` pair.
fn read_counted<'a>(r: &mut Reader<'a>, unicode: bool) -> Result<ShellStr<'a>> {
    // Widen first. See the module docs: doing this multiply in u16 is the bug.
    let count = usize::from(r.u16_le()?);
    let bytes = if unicode {
        count
            .checked_mul(2)
            .ok_or_else(|| r.err(ErrorKind::TooLarge))?
    } else {
        count
    };
    let data = r.take(bytes)?;
    Ok(if unicode {
        ShellStr::Utf16(data)
    } else {
        ShellStr::Ansi(data)
    })
}

/// MS-SHLLINK 2.4, new in revision 10.0: every `StringData` field except
/// `COMMAND_LINE_ARGUMENTS` "MUST NOT be greater than 260" characters.
///
/// A write-side constraint, not a read-side one. Links written before revision
/// 10.0 predate the rule and exceed it, so [`StringData::parse`] does not
/// enforce it; the builder does.
pub const MAX_STRING_CHARACTERS: usize = 260;

/// Reject a string the spec's length limit forbids writing.
///
/// Only the builder needs this; the reader deliberately does not enforce it.
#[cfg(feature = "alloc")]
pub(crate) fn check_writable_length(count: usize, bounded: bool) -> Result<u16> {
    if bounded && count > MAX_STRING_CHARACTERS {
        return Err(rclip_core::Error::new(ErrorKind::TooLarge, 0));
    }
    u16::try_from(count).map_err(|_| rclip_core::Error::new(ErrorKind::TooLarge, 0))
}

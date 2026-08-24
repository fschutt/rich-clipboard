//! `CFURL` resource and volume property flags.
//!
//! Keys [`crate::key::TARGET_FLAGS`] and [`crate::key::VOLUME_FLAGS`] both hold
//! a 24-byte `0x0201` data record laid out as three little-endian `u64`s: the
//! flag values, the mask of flags the creator asked for, and eight reserved
//! bytes. Reading only the first `u64` and ignoring the mask is the usual
//! mistake — a clear bit means "false" only if the same bit is set in the mask,
//! otherwise it means "nobody asked, so nothing was recorded".

use rclip_core::{Error, ErrorKind, Reader, Result};

/// Bit values for [`crate::key::TARGET_FLAGS`], from `CFURLPriv.h`.
pub mod resource {
    pub const IS_REGULAR_FILE: u64 = 0x0000_0001;
    pub const IS_DIRECTORY: u64 = 0x0000_0002;
    pub const IS_SYMBOLIC_LINK: u64 = 0x0000_0004;
    pub const IS_VOLUME: u64 = 0x0000_0008;
    pub const IS_PACKAGE: u64 = 0x0000_0010;
    pub const IS_SYSTEM_IMMUTABLE: u64 = 0x0000_0020;
    pub const IS_USER_IMMUTABLE: u64 = 0x0000_0040;
    pub const IS_HIDDEN: u64 = 0x0000_0080;
    pub const HAS_HIDDEN_EXTENSION: u64 = 0x0000_0100;
    pub const IS_APPLICATION: u64 = 0x0000_0200;
    pub const IS_COMPRESSED: u64 = 0x0000_0400;
    pub const CAN_SET_HIDDEN_EXTENSION: u64 = 0x0000_0800;
    pub const IS_READABLE: u64 = 0x0000_1000;
    pub const IS_WRITEABLE: u64 = 0x0000_2000;
    pub const IS_EXECUTABLE: u64 = 0x0000_4000;
    /// The target is itself a Finder alias file.
    pub const IS_ALIAS_FILE: u64 = 0x0000_8000;
    pub const IS_MOUNT_TRIGGER: u64 = 0x0001_0000;
}

/// Bit values for [`crate::key::VOLUME_FLAGS`], from `CFURLPriv.h`. Only the
/// handful that a clipboard consumer plausibly cares about are named; the rest
/// are readable through [`Flags::raw`].
pub mod volume {
    pub const IS_LOCAL: u64 = 0x0000_0001;
    pub const IS_AUTOMOUNT: u64 = 0x0000_0002;
    pub const DONT_BROWSE: u64 = 0x0000_0004;
    pub const IS_READ_ONLY: u64 = 0x0000_0008;
    pub const IS_QUARANTINED: u64 = 0x0000_0010;
    pub const IS_EJECTABLE: u64 = 0x0000_0020;
    pub const IS_REMOVABLE: u64 = 0x0000_0040;
    pub const IS_INTERNAL: u64 = 0x0000_0080;
    pub const IS_EXTERNAL: u64 = 0x0000_0100;
    pub const IS_DISK_IMAGE: u64 = 0x0000_0200;
    pub const IS_FILE_VAULT: u64 = 0x0000_0400;
}

/// A flags/mask pair out of a `0x1010` or `0x2020` record.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Flags {
    /// The flag bits themselves.
    pub raw: u64,
    /// Which bits the bookmark's creator asked to be recorded. A bit that is
    /// clear here was never sampled, so its value in [`Flags::raw`] is zero by
    /// default rather than by observation.
    pub asked_for: u64,
}

impl Flags {
    /// Parse the 24-byte payload of a flags record.
    ///
    /// Accepts a payload of 16 bytes as well: the trailing eight reserved bytes
    /// are omitted by at least one third-party bookmark writer, and dropping an
    /// otherwise-usable bookmark over eight zero bytes is not worth it.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 16 {
            return Err(Error::new(ErrorKind::BadLength, 0));
        }
        let mut r = Reader::new(data);
        let raw = r.u64_le()?;
        let asked_for = r.u64_le()?;
        Ok(Self { raw, asked_for })
    }

    /// `true` if every bit in `mask` is set.
    #[must_use]
    pub const fn has(self, mask: u64) -> bool {
        self.raw & mask == mask
    }

    /// `true` if the creator asked for every bit in `mask`, i.e. whether
    /// [`Flags::has`] is answering from data or from the default.
    #[must_use]
    pub const fn was_asked_for(self, mask: u64) -> bool {
        self.asked_for & mask == mask
    }

    /// Convenience for the resource flag that matters most to a file drop.
    #[must_use]
    pub const fn is_directory(self) -> bool {
        self.has(resource::IS_DIRECTORY)
    }

    #[must_use]
    pub const fn is_regular_file(self) -> bool {
        self.has(resource::IS_REGULAR_FILE)
    }

    #[must_use]
    pub const fn is_symbolic_link(self) -> bool {
        self.has(resource::IS_SYMBOLIC_LINK)
    }

    /// `true` when the bookmark's target is itself a Finder alias file, which
    /// means resolving it once still leaves you pointing at an alias.
    #[must_use]
    pub const fn is_alias_file(self) -> bool {
        self.has(resource::IS_ALIAS_FILE)
    }

    #[must_use]
    pub const fn is_package(self) -> bool {
        self.has(resource::IS_PACKAGE)
    }
}

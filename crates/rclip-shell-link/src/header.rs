//! MS-SHLLINK 2.1 — `ShellLinkHeader`, and the flag words in it.

use core::fmt;

use rclip_core::{Error, ErrorKind, Reader, Result};

use crate::filetime::FileTime;

/// `HeaderSize`. The spec says MUST be `0x0000004C`; it is also the actual size
/// of the structure, so it doubles as the offset of whatever follows.
pub const HEADER_SIZE: usize = 0x0000_004C;

/// `LinkCLSID`, `00021401-0000-0000-C000-000000000046`, in packet
/// representation.
pub const LINK_CLSID: [u8; 16] = [
    0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

/// MS-SHLLINK 2.1 — the fixed 76-byte header every shell link starts with.
///
/// ```text
/// 0x00  u32   HeaderSize, 0x4C
/// 0x04  [16]  LinkCLSID
/// 0x14  u32   LinkFlags
/// 0x18  u32   FileAttributes
/// 0x1C  u64   CreationTime    (FILETIME)
/// 0x24  u64   AccessTime
/// 0x2C  u64   WriteTime
/// 0x34  u32   FileSize        (low 32 bits)
/// 0x38  i32   IconIndex       (signed)
/// 0x3C  u32   ShowCommand
/// 0x40  u16   HotKey
/// 0x42  u16   Reserved1       MUST be zero
/// 0x44  u32   Reserved2       MUST be zero
/// 0x48  u32   Reserved3       MUST be zero
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct ShellLinkHeader {
    /// Which optional sections follow, and how strings in them are encoded.
    pub link_flags: LinkFlags,
    /// The link target's attributes, as of when the link was written.
    pub file_attributes: FileAttributes,
    pub creation_time: FileTime,
    pub access_time: FileTime,
    pub write_time: FileTime,
    /// Low 32 bits of the target's size. A target above 4 GiB stores only the
    /// least significant word, so this is a hint and not a size.
    pub file_size: u32,
    /// Signed index into the icon location. Negative values are a resource ID
    /// rather than an index, which is why this is `i32` and not `u32`.
    pub icon_index: i32,
    pub show_command: ShowCommand,
    pub hot_key: HotKey,
}

impl ShellLinkHeader {
    /// Parse the header from the start of `buf`.
    ///
    /// Rejects a wrong `HeaderSize` and a wrong `LinkCLSID`, and nothing else.
    /// In particular it does **not** reject unrecognised `LinkFlags` or
    /// `FileAttributes` bits: five `LinkFlags` bits are formally undefined, two
    /// more are named `Unused`, and refusing a file because a future Windows set
    /// one would be a self-inflicted compatibility break. Unknown bits are kept
    /// and can be read back with [`LinkFlags::unknown_bits`].
    pub fn parse(buf: &[u8]) -> Result<Self> {
        let mut r = Reader::new(buf);

        let header_size = r.u32_le()?;
        if header_size as usize != HEADER_SIZE {
            return Err(Error::new(ErrorKind::BadLength, 0));
        }
        let clsid = r.guid()?;
        if clsid != LINK_CLSID {
            // A wrong CLSID is the signal that this is not a shell link at all,
            // which matters: a .lnk parser is the last place you want to be
            // guessing about what you were handed.
            return Err(Error::new(ErrorKind::BadMagic, 4));
        }

        let link_flags = LinkFlags(r.u32_le()?);
        let file_attributes = FileAttributes(r.u32_le()?);
        let creation_time = FileTime::read(&mut r)?;
        let access_time = FileTime::read(&mut r)?;
        let write_time = FileTime::read(&mut r)?;
        let file_size = r.u32_le()?;
        let icon_index = r.i32_le()?;
        let show_command = ShowCommand(r.u32_le()?);
        let hot_key = HotKey {
            key: r.u8()?,
            modifiers: r.u8()?,
        };

        // Reserved1 (u16) and Reserved2/3 (u32) follow. The spec says they MUST
        // be zero; real files honour that, but a non-zero reserved field is not
        // a reason to refuse a link, so they are skipped rather than checked.
        r.skip(10)?;
        debug_assert_eq!(r.pos(), HEADER_SIZE);

        Ok(Self {
            link_flags,
            file_attributes,
            creation_time,
            access_time,
            write_time,
            file_size,
            icon_index,
            show_command,
            hot_key,
        })
    }

    /// Serialize back to the 76 wire bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        out[0..4].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        out[4..20].copy_from_slice(&LINK_CLSID);
        out[20..24].copy_from_slice(&self.link_flags.0.to_le_bytes());
        out[24..28].copy_from_slice(&self.file_attributes.0.to_le_bytes());
        out[28..36].copy_from_slice(&self.creation_time.to_le_bytes());
        out[36..44].copy_from_slice(&self.access_time.to_le_bytes());
        out[44..52].copy_from_slice(&self.write_time.to_le_bytes());
        out[52..56].copy_from_slice(&self.file_size.to_le_bytes());
        out[56..60].copy_from_slice(&self.icon_index.to_le_bytes());
        out[60..64].copy_from_slice(&self.show_command.0.to_le_bytes());
        out[64] = self.hot_key.key;
        out[65] = self.hot_key.modifiers;
        // Reserved1/2/3 stay zero.
        out
    }
}

/// MS-SHLLINK 2.1.1 — which optional sections are present, and how strings are
/// encoded.
///
/// A plain newtype rather than a `bitflags!` macro: the crate would be a
/// dependency for four lines of code, and the one thing worth having here — a
/// `Debug` that names the set bits — the macro does not give you in a form that
/// survives unknown bits.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct LinkFlags(pub u32);

impl LinkFlags {
    /// A `LinkTargetIDList` follows the header.
    pub const HAS_LINK_TARGET_ID_LIST: Self = Self(0x0000_0001);
    /// A `LinkInfo` structure is present.
    pub const HAS_LINK_INFO: Self = Self(0x0000_0002);
    /// A `NAME_STRING` is present in `StringData`.
    pub const HAS_NAME: Self = Self(0x0000_0004);
    /// A `RELATIVE_PATH` is present.
    pub const HAS_RELATIVE_PATH: Self = Self(0x0000_0008);
    /// A `WORKING_DIR` is present.
    pub const HAS_WORKING_DIR: Self = Self(0x0000_0010);
    /// A `COMMAND_LINE_ARGUMENTS` string is present.
    pub const HAS_ARGUMENTS: Self = Self(0x0000_0020);
    /// An `ICON_LOCATION` is present.
    pub const HAS_ICON_LOCATION: Self = Self(0x0000_0040);
    /// `StringData` is UTF-16LE. If clear, it is in the *writer's* system
    /// default code page, which is not recorded anywhere in the file.
    pub const IS_UNICODE: Self = Self(0x0000_0080);
    /// The `LinkInfo` structure is to be ignored on resolution.
    pub const FORCE_NO_LINK_INFO: Self = Self(0x0000_0100);
    /// An `EnvironmentVariableDataBlock` is present.
    pub const HAS_EXP_STRING: Self = Self(0x0000_0200);
    /// Run a 16-bit target in a separate virtual machine.
    pub const RUN_IN_SEPARATE_PROCESS: Self = Self(0x0000_0400);
    /// Undefined; MUST be ignored.
    pub const UNUSED1: Self = Self(0x0000_0800);
    /// A `DarwinDataBlock` is present.
    pub const HAS_DARWIN_ID: Self = Self(0x0000_1000);
    /// Activate the target as a different user.
    pub const RUN_AS_USER: Self = Self(0x0000_2000);
    /// An `IconEnvironmentDataBlock` is present.
    pub const HAS_EXP_ICON: Self = Self(0x0000_4000);
    /// Represent the file system location in the shell namespace when parsing
    /// the path into an IDList.
    pub const NO_PIDL_ALIAS: Self = Self(0x0000_8000);
    /// Undefined; MUST be ignored.
    pub const UNUSED2: Self = Self(0x0001_0000);
    /// A `ShimDataBlock` is present.
    pub const RUN_WITH_SHIM_LAYER: Self = Self(0x0002_0000);
    /// The `TrackerDataBlock` is to be ignored.
    pub const FORCE_NO_LINK_TRACK: Self = Self(0x0004_0000);
    /// Collect target properties into the `PropertyStoreDataBlock`.
    pub const ENABLE_TARGET_METADATA: Self = Self(0x0008_0000);
    /// The `EnvironmentVariableDataBlock` is to be ignored.
    pub const DISABLE_LINK_PATH_TRACKING: Self = Self(0x0010_0000);
    /// The `SpecialFolderDataBlock` and `KnownFolderDataBlock` are to be
    /// ignored on load, and should not be written.
    pub const DISABLE_KNOWN_FOLDER_TRACKING: Self = Self(0x0020_0000);
    /// Use the unaliased form of the known folder IDList on load.
    pub const DISABLE_KNOWN_FOLDER_ALIAS: Self = Self(0x0040_0000);
    /// A link may reference another link.
    pub const ALLOW_LINK_TO_LINK: Self = Self(0x0080_0000);
    /// Prefer the unaliased known folder form when saving.
    pub const UNALIAS_ON_SAVE: Self = Self(0x0100_0000);
    /// Refer to the target through the `EnvironmentVariableDataBlock` path
    /// rather than storing a target IDList.
    pub const PREFER_ENVIRONMENT_PATH: Self = Self(0x0200_0000);
    /// For a UNC target that is really local, keep the local IDList in the
    /// `PropertyStoreDataBlock`.
    pub const KEEP_LOCAL_ID_LIST_FOR_UNC_TARGET: Self = Self(0x0400_0000);

    /// Every bit the spec assigns a name to. Bits 27-31 are undefined.
    pub const DEFINED: Self = Self(0x07FF_FFFF);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Bits set here that the spec does not define.
    ///
    /// Not an error — see [`ShellLinkHeader::parse`] — but worth surfacing,
    /// because a link with undefined bits set is either from a newer Windows or
    /// hand-built, and both are interesting.
    #[must_use]
    pub const fn unknown_bits(self) -> u32 {
        self.0 & !Self::DEFINED.0
    }

    /// Whether `StringData` in this link is UTF-16LE.
    #[must_use]
    pub const fn is_unicode(self) -> bool {
        self.contains(Self::IS_UNICODE)
    }
}

impl core::ops::BitOr for LinkFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl core::ops::BitOrAssign for LinkFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

static LINK_FLAG_NAMES: &[(u32, &str)] = &[
    (0x0000_0001, "HasLinkTargetIDList"),
    (0x0000_0002, "HasLinkInfo"),
    (0x0000_0004, "HasName"),
    (0x0000_0008, "HasRelativePath"),
    (0x0000_0010, "HasWorkingDir"),
    (0x0000_0020, "HasArguments"),
    (0x0000_0040, "HasIconLocation"),
    (0x0000_0080, "IsUnicode"),
    (0x0000_0100, "ForceNoLinkInfo"),
    (0x0000_0200, "HasExpString"),
    (0x0000_0400, "RunInSeparateProcess"),
    (0x0000_0800, "Unused1"),
    (0x0000_1000, "HasDarwinID"),
    (0x0000_2000, "RunAsUser"),
    (0x0000_4000, "HasExpIcon"),
    (0x0000_8000, "NoPidlAlias"),
    (0x0001_0000, "Unused2"),
    (0x0002_0000, "RunWithShimLayer"),
    (0x0004_0000, "ForceNoLinkTrack"),
    (0x0008_0000, "EnableTargetMetadata"),
    (0x0010_0000, "DisableLinkPathTracking"),
    (0x0020_0000, "DisableKnownFolderTracking"),
    (0x0040_0000, "DisableKnownFolderAlias"),
    (0x0080_0000, "AllowLinkToLink"),
    (0x0100_0000, "UnaliasOnSave"),
    (0x0200_0000, "PreferEnvironmentPath"),
    (0x0400_0000, "KeepLocalIDListForUNCTarget"),
];

impl fmt::Debug for LinkFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LinkFlags({:#010X}", self.0)?;
        let mut first = true;
        for (bit, name) in LINK_FLAG_NAMES {
            if self.0 & bit != 0 {
                f.write_str(if first { ": " } else { " | " })?;
                f.write_str(name)?;
                first = false;
            }
        }
        if self.unknown_bits() != 0 {
            write!(
                f,
                "{} undefined:{:#X}",
                if first { ":" } else { " |" },
                self.unknown_bits()
            )?;
        }
        f.write_str(")")
    }
}

/// MS-SHLLINK 2.1.2 — `FILE_ATTRIBUTE_*` bits for the link target.
///
/// Only the low 15 bits are defined; bits 3 and 6 are reserved and MUST be zero.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct FileAttributes(pub u32);

impl FileAttributes {
    pub const READONLY: Self = Self(0x0000_0001);
    pub const HIDDEN: Self = Self(0x0000_0002);
    pub const SYSTEM: Self = Self(0x0000_0004);
    pub const DIRECTORY: Self = Self(0x0000_0010);
    pub const ARCHIVE: Self = Self(0x0000_0020);
    /// If set, every other bit MUST be clear.
    pub const NORMAL: Self = Self(0x0000_0080);
    pub const TEMPORARY: Self = Self(0x0000_0100);
    pub const SPARSE_FILE: Self = Self(0x0000_0200);
    pub const REPARSE_POINT: Self = Self(0x0000_0400);
    pub const COMPRESSED: Self = Self(0x0000_0800);
    pub const OFFLINE: Self = Self(0x0000_1000);
    pub const NOT_CONTENT_INDEXED: Self = Self(0x0000_2000);
    pub const ENCRYPTED: Self = Self(0x0000_4000);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn is_directory(self) -> bool {
        self.contains(Self::DIRECTORY)
    }
}

impl core::ops::BitOr for FileAttributes {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

static FILE_ATTRIBUTE_NAMES: &[(u32, &str)] = &[
    (0x0000_0001, "READONLY"),
    (0x0000_0002, "HIDDEN"),
    (0x0000_0004, "SYSTEM"),
    (0x0000_0010, "DIRECTORY"),
    (0x0000_0020, "ARCHIVE"),
    (0x0000_0080, "NORMAL"),
    (0x0000_0100, "TEMPORARY"),
    (0x0000_0200, "SPARSE_FILE"),
    (0x0000_0400, "REPARSE_POINT"),
    (0x0000_0800, "COMPRESSED"),
    (0x0000_1000, "OFFLINE"),
    (0x0000_2000, "NOT_CONTENT_INDEXED"),
    (0x0000_4000, "ENCRYPTED"),
];

impl fmt::Debug for FileAttributes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileAttributes({:#010X}", self.0)?;
        let mut first = true;
        for (bit, name) in FILE_ATTRIBUTE_NAMES {
            if self.0 & bit != 0 {
                f.write_str(if first { ": " } else { " | " })?;
                f.write_str(name)?;
                first = false;
            }
        }
        f.write_str(")")
    }
}

/// MS-SHLLINK 2.1 `ShowCommand` — the window state to launch the target in.
///
/// A newtype rather than an enum so the raw value survives a round trip. The
/// spec defines three values and says all others MUST be treated as
/// `SW_SHOWNORMAL`; [`ShowCommand::effective`] applies that rule without
/// destroying what was actually on disk.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct ShowCommand(pub u32);

impl ShowCommand {
    /// `SW_SHOWNORMAL`.
    pub const NORMAL: Self = Self(0x0000_0001);
    /// `SW_SHOWMAXIMIZED`.
    pub const MAXIMIZED: Self = Self(0x0000_0003);
    /// `SW_SHOWMINNOACTIVE`.
    pub const MIN_NO_ACTIVE: Self = Self(0x0000_0007);

    /// The value after applying the spec's "all other values MUST be treated as
    /// `SW_SHOWNORMAL`" rule.
    #[must_use]
    pub const fn effective(self) -> Self {
        match self.0 {
            0x0000_0003 => Self::MAXIMIZED,
            0x0000_0007 => Self::MIN_NO_ACTIVE,
            _ => Self::NORMAL,
        }
    }
}

impl Default for ShowCommand {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl fmt::Debug for ShowCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.0 {
            0x0000_0001 => "SW_SHOWNORMAL",
            0x0000_0003 => "SW_SHOWMAXIMIZED",
            0x0000_0007 => "SW_SHOWMINNOACTIVE",
            _ => "other (treated as SW_SHOWNORMAL)",
        };
        write!(f, "ShowCommand({:#X}: {name})", self.0)
    }
}

/// MS-SHLLINK 2.1.3 — the shortcut key assigned to the link.
///
/// `key` is a virtual key code and `modifiers` a `HOTKEYF_*` mask. Both zero
/// means no hotkey, which is the overwhelmingly common case and which the
/// seeding template treated as a parse error.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct HotKey {
    /// Virtual key code: `0x30`-`0x39` digits, `0x41`-`0x5A` letters,
    /// `0x70`-`0x87` F1-F24, `0x90` NumLock, `0x91` ScrollLock.
    pub key: u8,
    /// `HOTKEYF_*` bits.
    pub modifiers: u8,
}

impl HotKey {
    pub const SHIFT: u8 = 0x01;
    pub const CONTROL: u8 = 0x02;
    pub const ALT: u8 = 0x04;

    #[must_use]
    pub const fn is_unset(self) -> bool {
        self.key == 0 && self.modifiers == 0
    }

    #[must_use]
    pub const fn has_shift(self) -> bool {
        self.modifiers & Self::SHIFT != 0
    }

    #[must_use]
    pub const fn has_control(self) -> bool {
        self.modifiers & Self::CONTROL != 0
    }

    #[must_use]
    pub const fn has_alt(self) -> bool {
        self.modifiers & Self::ALT != 0
    }

    /// The printable character for a digit or letter key.
    #[must_use]
    pub const fn key_char(self) -> Option<char> {
        match self.key {
            b'0'..=b'9' | b'A'..=b'Z' => Some(self.key as char),
            _ => None,
        }
    }

    /// `1`-`24` for `VK_F1`-`VK_F24`.
    #[must_use]
    pub const fn function_key(self) -> Option<u8> {
        match self.key {
            0x70..=0x87 => Some(self.key - 0x70 + 1),
            _ => None,
        }
    }
}

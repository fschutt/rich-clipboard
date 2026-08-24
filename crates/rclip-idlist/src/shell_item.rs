//! Interpreting the body of a `SHITEMID`.
//!
//! None of this is documented by Microsoft. The layouts below follow libyal's
//! [Windows Shell Item format][libfwsi] document *and* the `libfwsi` C parser,
//! which disagree in places; where they do, this crate follows the code, since
//! that is what has been run against real captures. Each such case is called out
//! at the field it affects.
//!
//! [libfwsi]: https://github.com/libyal/libfwsi/tree/main/documentation
//!
//! # Nothing in this module returns an error
//!
//! Every function here is infallible and every variant keeps the raw bytes it
//! was parsed from. That is the design, not laziness: a shell item can come from
//! any namespace extension installed on the machine that copied it, so "a class
//! byte I have never seen" is a normal Tuesday and not a parse failure. Losing
//! one segment of a breadcrumb is survivable; refusing the paste is not.
//!
//! Fields whose meaning is uncertain are named `unknown*` or left out entirely
//! rather than guessed at.
//!
//! # Offsets in this file
//!
//! The forensics literature counts offsets from the start of the *item*,
//! including the two bytes of `cb`. This crate parses `abID`, which begins two
//! bytes later, so every documented offset appears here minus two. The doc
//! comments give the `abID`-relative numbers to match the code.

use crate::{dostime::DosDateTime, guid::Guid, item::MIN_ITEM_SIZE, string::ShellStr};

/// Mask that selects the class *family* from a class type indicator.
///
/// `0x70`, not `0xF0`: bit `0x80` is a per-family flag, not part of the family.
/// That is why `0xB1` is a file entry (`0xB1 & 0x70 == 0x30`) and `0xC3` a
/// network location. Masking with `0xF0` — the obvious-looking choice — drops
/// both on the floor.
pub const CLASS_FAMILY_MASK: u8 = 0x70;

/// Root folder / shell folder identifier item. Class `0x1F` exactly.
pub const CLASS_ROOT_FOLDER: u8 = 0x1F;
/// Family `0x20`: volumes and drives.
pub const CLASS_FAMILY_VOLUME: u8 = 0x20;
/// Family `0x30`: file entries — files and directories.
pub const CLASS_FAMILY_FILE_ENTRY: u8 = 0x30;
/// Family `0x40`: network locations — shares, servers, workgroups.
pub const CLASS_FAMILY_NETWORK: u8 = 0x40;
/// Family `0x60`. Only `0x61`, [`CLASS_URI`], is defined within it.
pub const CLASS_FAMILY_URI: u8 = 0x60;
/// URI item. Class `0x61` exactly.
pub const CLASS_URI: u8 = 0x61;
/// Control panel item. Class `0x71` exactly.
// TODO(phase-3): decode the control panel item body (a GUID plus a name).
pub const CLASS_CONTROL_PANEL: u8 = 0x71;

/// Volume item whose body is a GUID rather than a drive name.
pub const CLASS_VOLUME_GUID: u8 = 0x2E;

/// File entry class flag: the item is a directory.
pub const FILE_ENTRY_DIRECTORY: u8 = 0x01;
/// File entry class flag: the item is a file.
pub const FILE_ENTRY_FILE: u8 = 0x02;
/// File entry class flag: the primary name is UTF-16LE rather than code page
/// bytes. `0x31`/`0x32` are an ANSI directory/file, `0x35`/`0x36` the Unicode
/// pair.
pub const FILE_ENTRY_UNICODE: u8 = 0x04;
/// File entry class flag: a `0xBEEF0003` block with a class identifier follows.
pub const FILE_ENTRY_HAS_CLSID: u8 = 0x80;

/// Network location flag: a description string follows the location.
pub const NETWORK_HAS_DESCRIPTION: u8 = 0x80;
/// Network location flag: a comment string follows.
pub const NETWORK_HAS_COMMENT: u8 = 0x40;

/// URI flag: the strings in this item are UTF-16LE.
pub const URI_UNICODE: u8 = 0x80;

/// High half of every known extension block signature.
pub const EXTENSION_SIGNATURE_PREFIX: u32 = 0xBEEF_0000;
/// The extension block carrying a file entry's long (non-8.3) name.
pub const EXTENSION_FILE_ENTRY: u32 = 0xBEEF_0004;
/// Smallest extension block `libfwsi` will accept. Below this the block cannot
/// hold its own header plus the trailing offset field.
pub const MIN_EXTENSION_SIZE: usize = 10;

/// A shell item body, interpreted as far as it can be.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShellItem<'a> {
    /// `cb == 2`: an item with no body. Legal, and used as a "the desktop"
    /// placeholder at the head of some lists.
    Empty,
    /// Class `0x1F`.
    RootFolder(RootFolder<'a>),
    /// Family `0x20`.
    Volume(Volume<'a>),
    /// Family `0x30`, including `0xB1`.
    FileEntry(FileEntry<'a>),
    /// Family `0x40`, including `0xC3`.
    NetworkLocation(NetworkLocation<'a>),
    /// Class `0x61`.
    Uri(Uri<'a>),
    /// Anything else, or a body too short for the layout its class implies.
    ///
    /// Not an error, and not a dead end — `raw` is the complete `abID`, so a
    /// caller that knows the extension can decode it.
    Unknown {
        /// The class type indicator, i.e. the first byte of `abID`.
        class: u8,
        /// The whole body, unmodified.
        raw: &'a [u8],
    },
}

impl<'a> ShellItem<'a> {
    /// Interpret `data`, the `abID` of a `SHITEMID`.
    ///
    /// Dispatch is on the class byte alone. `libfwsi` additionally probes a set
    /// of 32-bit signatures at fixed offsets to recognise MTP devices, control
    /// panel categories, Acronis images and the like before falling back to the
    /// class byte; those all land in [`ShellItem::Unknown`] here.
    // TODO(phase-3): signature-based recognition (MTP, users property view,
    // delegate folders) and the compressed-folder heuristics.
    #[must_use]
    pub fn parse(data: &'a [u8]) -> Self {
        let Some(&class) = data.first() else {
            return Self::Empty;
        };
        let parsed = match class & CLASS_FAMILY_MASK {
            // 0x10 is only a root folder for the exact value 0x1F.
            0x10 if class == CLASS_ROOT_FOLDER => RootFolder::parse(data).map(Self::RootFolder),
            CLASS_FAMILY_VOLUME => Volume::parse(class, data).map(Self::Volume),
            CLASS_FAMILY_FILE_ENTRY => FileEntry::parse(class, data).map(Self::FileEntry),
            CLASS_FAMILY_NETWORK => NetworkLocation::parse(class, data).map(Self::NetworkLocation),
            CLASS_FAMILY_URI if class == CLASS_URI => Uri::parse(class, data).map(Self::Uri),
            _ => None,
        };
        parsed.unwrap_or(Self::Unknown { class, raw: data })
    }

    /// The class type indicator, or `None` for an empty item.
    #[must_use]
    pub const fn class(&self) -> Option<u8> {
        match self {
            Self::Empty => None,
            Self::RootFolder(_) => Some(CLASS_ROOT_FOLDER),
            Self::Volume(v) => Some(v.class),
            Self::FileEntry(f) => Some(f.class),
            Self::NetworkLocation(n) => Some(n.class),
            Self::Uri(u) => Some(u.class),
            Self::Unknown { class, .. } => Some(*class),
        }
    }

    /// The complete `abID` this item was parsed from.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        match self {
            Self::Empty => &[],
            Self::RootFolder(r) => r.raw,
            Self::Volume(v) => v.raw,
            Self::FileEntry(f) => f.raw,
            Self::NetworkLocation(n) => n.raw,
            Self::Uri(u) => u.raw,
            Self::Unknown { raw, .. } => raw,
        }
    }

    /// The best name this item can offer for a breadcrumb, if it has one.
    ///
    /// For a file entry that means the long name from the `0xBEEF0004`
    /// extension block when it is there and the primary name when it is not —
    /// the primary name is frequently the 8.3 form, so preferring the long name
    /// is the difference between `Program Files` and `PROGRA~1`.
    ///
    /// For a root folder it means the shell's name for the GUID, when the GUID
    /// is one of the well-known ones — that name is a `&'static str` rather than
    /// a view into the input, which is why this returns [`ShellStr`] and not a
    /// slice tied to the item.
    ///
    /// The result is a **label**. It is not a path component, nothing has
    /// validated it, and it may contain any character at all — a path
    /// separator, `..`, or a right-to-left override that makes `exe.txt` read
    /// as `txt.exe`. Do not concatenate it into a path and open the result.
    #[must_use]
    pub fn display_name(&self) -> Option<ShellStr<'a>> {
        match self {
            Self::FileEntry(f) => f.long_name.or(Some(f.primary_name)),
            Self::Volume(v) => v.name,
            Self::NetworkLocation(n) => Some(n.location),
            Self::Uri(u) => u.uri,
            // A GUID we do not recognise gets no name rather than its own hex:
            // `{F02C1A0D-...}` in a breadcrumb is worse than a gap.
            Self::RootFolder(r) => r
                .guid
                .well_known_name()
                .map(|n| ShellStr::Ansi(n.as_bytes())),
            Self::Empty | Self::Unknown { .. } => None,
        }
    }
}

/// Class `0x1F`: a shell folder identified by GUID — `My Computer`,
/// `Recycle Bin`, `Control Panel`.
///
/// ```text
/// abID[0]      class type indicator, 0x1F
/// abID[1]      sort index
/// abID[2..18]  shell folder GUID, packet representation
/// ```
///
/// 18 bytes of body, so `cb == 20`. A larger `cb` means extension blocks follow
/// (`0xBEEF0017` and `0xBEEF0026` have been seen here).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct RootFolder<'a> {
    /// Where the shell sorts this entry: `0x42` Libraries, `0x48` My Documents,
    /// `0x50` My Computer, `0x58` Network, `0x60` Recycle Bin. Mirrors
    /// `SortOrderIndex` under `HKCR\CLSID\{...}`. Not load bearing.
    pub sort_index: u8,
    /// The folder's class ID. Try [`Guid::well_known_name`] before showing it.
    pub guid: Guid,
    /// The entire body including the class byte. Longer than 18 bytes when
    /// extension blocks follow the GUID.
    pub raw: &'a [u8],
}

impl<'a> RootFolder<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        Some(Self {
            sort_index: *data.get(1)?,
            guid: Guid::from_slice(data.get(2..)?)?,
            raw: data,
        })
    }
}

/// Family `0x20`: a volume, usually a drive letter.
///
/// Two layouts. The named form (`0x23`, `0x25`, `0x29`, `0x2A`, `0x2F`):
///
/// ```text
/// abID[0]       class type indicator
/// abID[1..21]   volume name, ASCII, NUL-terminated, zero-padded to 20 bytes
/// abID[21..23]  unknown u16
/// abID[23..39]  shell folder GUID, if the item is long enough
/// ```
///
/// and the GUID form, `0x2E` only:
///
/// ```text
/// abID[0]       0x2E
/// abID[1]       unknown / flags
/// abID[2..18]   GUID
/// ```
///
/// The libfwsi *document* says the split is on class flag `0x01` ("has name"),
/// but the libfwsi *code* special-cases `0x2E` and treats everything else as
/// named. The two disagree for `0x2A`, whose `0x01` bit is clear yet which
/// carries a name in practice. This follows the code.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Volume<'a> {
    pub class: u8,
    /// The volume path, e.g. `C:\`, for the named form.
    pub name: Option<ShellStr<'a>>,
    /// The folder GUID, for the `0x2E` form or a long named item.
    pub guid: Option<Guid>,
    /// The entire body including the class byte.
    pub raw: &'a [u8],
}

/// Class values in the volume family whose body is a name. Anything else in the
/// family that is not [`CLASS_VOLUME_GUID`] is returned with `name: None`.
const VOLUME_NAMED: [u8; 5] = [0x23, 0x25, 0x29, 0x2A, 0x2F];

impl<'a> Volume<'a> {
    fn parse(class: u8, data: &'a [u8]) -> Option<Self> {
        if class == CLASS_VOLUME_GUID {
            return Some(Self {
                class,
                name: None,
                guid: data.get(2..).and_then(Guid::from_slice),
                raw: data,
            });
        }
        if VOLUME_NAMED.contains(&class) {
            let field = data.get(1..21)?;
            return Some(Self {
                class,
                name: Some(ShellStr::Ansi(nul_terminated(field))),
                // 23..39 only exists on the longer form; absent is normal.
                guid: data
                    .get(23..)
                    .and_then(Guid::from_slice)
                    .filter(|g| *g.as_bytes() != [0; 16]),
                raw: data,
            });
        }
        // A class in the family that neither the document nor the code covers.
        // Still a volume, still worth returning: the bytes are all here.
        Some(Self {
            class,
            name: None,
            guid: None,
            raw: data,
        })
    }
}

/// Family `0x30`: a file or directory.
///
/// ```text
/// abID[0]       class type indicator
/// abID[1]       unknown, zero in every sample
/// abID[2..6]    u32 file size, low 32 bits; 0 for a directory
/// abID[6..10]   FAT date/time, last modified
/// abID[10..12]  u16 file attribute flags (FILE_ATTRIBUTE_*)
/// abID[12..]    primary name, NUL-terminated; UTF-16LE if class & 0x04
/// ..            padding to an even offset, then extension blocks
/// ```
///
/// The primary name is the 8.3 short name on anything written by a shell that
/// still generates one, which is exactly why the `0xBEEF0004` extension block
/// and the long name in it matter.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct FileEntry<'a> {
    pub class: u8,
    /// Low 32 bits of the size. Zero for directories, and zero on files above
    /// 4 GiB is indistinguishable from a real zero — the field is only 32 bits
    /// wide on the wire.
    pub file_size: u32,
    /// Last modification, in FAT's two-second-granularity encoding. Frequently
    /// unset; check [`DosDateTime::is_unset`].
    pub modified: DosDateTime,
    /// `FILE_ATTRIBUTE_*` bits — the low 16 of the Win32 set, so
    /// `FILE_ATTRIBUTE_VIRTUAL` and above cannot appear.
    pub attributes: u16,
    /// The name stored in the item body itself, frequently the 8.3 form.
    pub primary_name: ShellStr<'a>,
    /// The long name from the `0xBEEF0004` extension block, when present.
    /// Always UTF-16LE, and not necessarily *valid* UTF-16 — unpaired
    /// surrogates occur in real captures, which is why this is bytes and not
    /// `&str`.
    pub long_name: Option<ShellStr<'a>>,
    /// The localized display name from the same block: `Programme` for
    /// `Program Files` on a German install, or a `@shell32.dll,-21781`
    /// resource reference.
    pub localized_name: Option<ShellStr<'a>>,
    /// The first extension block, whatever its signature.
    pub extension: Option<ExtensionBlock<'a>>,
    /// The entire body including the class byte.
    pub raw: &'a [u8],
}

/// `FILE_ATTRIBUTE_DIRECTORY`, for cross-checking [`FileEntry::is_directory`].
pub const FILE_ATTRIBUTE_DIRECTORY: u16 = 0x0010;

impl<'a> FileEntry<'a> {
    /// `true` if the class byte says directory.
    ///
    /// Worth cross-checking against `attributes & `[`FILE_ATTRIBUTE_DIRECTORY`]:
    /// the two disagree in real captures often enough that trusting either
    /// alone is a bug.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.class & FILE_ENTRY_DIRECTORY != 0
    }

    #[must_use]
    pub const fn is_file(&self) -> bool {
        self.class & FILE_ENTRY_FILE != 0
    }

    /// `true` if the primary name is UTF-16LE rather than code page bytes.
    #[must_use]
    pub const fn has_unicode_name(&self) -> bool {
        self.class & FILE_ENTRY_UNICODE != 0
    }

    // TODO(phase-3): pre-XP file entries carry a *secondary* name (the 8.3 form)
    // after the primary one instead of an extension block. Detectable by the
    // u16 after the primary name being zero or larger than `cb`.
    fn parse(class: u8, data: &'a [u8]) -> Option<Self> {
        let file_size = u32::from_le_bytes(data.get(2..6)?.try_into().ok()?);
        let modified = DosDateTime::from_le_bytes(data.get(6..10)?.try_into().ok()?);
        let attributes = u16::from_le_bytes(data.get(10..12)?.try_into().ok()?);

        let name_bytes = data.get(12..)?;
        let primary_name = if class & FILE_ENTRY_UNICODE != 0 {
            ShellStr::Utf16(utf16_nul_terminated(name_bytes))
        } else {
            ShellStr::Ansi(nul_terminated(name_bytes))
        };

        let extension = ExtensionBlock::find(data);
        let file_ext = extension.and_then(ExtensionBlock::as_file_entry);

        Some(Self {
            class,
            file_size,
            modified,
            attributes,
            primary_name,
            long_name: file_ext.and_then(|e| e.long_name),
            localized_name: file_ext.and_then(|e| e.localized_name),
            extension,
            raw: data,
        })
    }
}

/// Family `0x40`: a network share, server or workgroup.
///
/// ```text
/// abID[0]   class type indicator; low nibble is the sub-type
/// abID[1]   unknown
/// abID[2]   flags
/// abID[3..] location, NUL-terminated ASCII (`\\server\share`)
/// ..        description, if flags & 0x80
/// ..        comment, if flags & 0x40
/// ```
///
/// Note the order: the description comes first even though its flag bit is the
/// higher one.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct NetworkLocation<'a> {
    pub class: u8,
    pub flags: u8,
    /// The network name or UNC path.
    pub location: ShellStr<'a>,
    /// Present when `flags & `[`NETWORK_HAS_DESCRIPTION`].
    pub description: Option<ShellStr<'a>>,
    /// Present when `flags & `[`NETWORK_HAS_COMMENT`].
    pub comment: Option<ShellStr<'a>>,
    pub raw: &'a [u8],
}

impl<'a> NetworkLocation<'a> {
    /// The low nibble of the class: `0x01` domain or workgroup, `0x02` a
    /// server's UNC path, `0x03` a share's UNC path, `0x06` Microsoft Windows
    /// Network, `0x07` Entire Network.
    #[must_use]
    pub const fn sub_type(&self) -> u8 {
        self.class & 0x0F
    }

    fn parse(class: u8, data: &'a [u8]) -> Option<Self> {
        let flags = *data.get(2)?;
        let mut rest = data.get(3..)?;

        let location = nul_terminated(rest);
        rest = advance_past_nul(rest, location.len());

        let description = if flags & NETWORK_HAS_DESCRIPTION != 0 {
            let s = nul_terminated(rest);
            rest = advance_past_nul(rest, s.len());
            Some(ShellStr::Ansi(s))
        } else {
            None
        };
        let comment = if flags & NETWORK_HAS_COMMENT != 0 {
            Some(ShellStr::Ansi(nul_terminated(rest)))
        } else {
            None
        };

        Some(Self {
            class,
            flags,
            location: ShellStr::Ansi(location),
            description,
            comment,
            raw: data,
        })
    }
}

/// Class `0x61`: a URI in the shell namespace.
///
/// ```text
/// abID[0]      0x61
/// abID[1]      flags; 0x80 means the strings are UTF-16LE
/// abID[2..4]   u16 size of the data block, not counting these two bytes
/// abID[4..]    data block
/// ..           URI string, NUL-terminated
/// ```
///
/// The data block's interior is version dependent — for the FTP variant it holds
/// a `FILETIME` and three length-prefixed strings including a **password** —
/// and is kept as [`Uri::data`] rather than decoded.
// TODO(phase-3): decode the >= 36 byte FTP data block. Note when doing so that
// it contains a cleartext password, which is a reason to keep it out of any
// Debug output.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Uri<'a> {
    pub class: u8,
    pub flags: u8,
    /// The version-dependent block between the header and the string.
    pub data: &'a [u8],
    /// The URI itself, when it could be located. The libfwsi document notes it
    /// is "not always present".
    pub uri: Option<ShellStr<'a>>,
    pub raw: &'a [u8],
}

impl<'a> Uri<'a> {
    fn parse(class: u8, data: &'a [u8]) -> Option<Self> {
        let flags = *data.get(1)?;
        let size = usize::from(u16::from_le_bytes(data.get(2..4)?.try_into().ok()?));
        let rest = data.get(4..)?;

        // `size` is off the wire. Clamp rather than fail: an over-long size
        // means a block written by a version we do not know, and the item is
        // still worth returning for its flags.
        let (fixed, tail) = rest.split_at(size.min(rest.len()));

        let uri = if tail.is_empty() {
            None
        } else if flags & URI_UNICODE != 0 {
            Some(ShellStr::Utf16(utf16_nul_terminated(tail)))
        } else {
            Some(ShellStr::Ansi(nul_terminated(tail)))
        };

        Some(Self {
            class,
            flags,
            data: fixed,
            uri,
            raw: data,
        })
    }
}

/// A `0xBEEFxxxx` extension block appended to a shell item.
///
/// Newer shells append these to items whose base layout was fixed by an older
/// one, which is why they are found from the *end* of the item rather than by
/// walking forward past a variable-length name: the last two bytes of a shell
/// item hold the offset of the first extension block, counted from the start of
/// the item including its `cb` field. Every block repeats that same trailing
/// field, so the chain stays self-describing.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ExtensionBlock<'a> {
    /// Offset of the block within `abID`.
    pub offset: usize,
    /// On-the-wire size of the whole block, including its own header.
    pub size: usize,
    /// Block version. The layout of the body depends on it — for
    /// `0xBEEF0004`, only `3`, `7`, `8` and `9` are known.
    pub version: u16,
    /// `0xBEEF0004` and friends.
    pub signature: u32,
    /// Everything after the 8-byte size/version/signature header, bounded by
    /// the block's own declared size.
    pub body: &'a [u8],
}

impl<'a> ExtensionBlock<'a> {
    /// Locate the first extension block in a shell item body.
    ///
    /// Every value used here came off the wire, so all of them are checked: the
    /// trailing offset must land inside the body and past the fixed header, the
    /// declared size must cover the block's own header and fit in what is left,
    /// and the signature must be a `0xBEEF` one.
    ///
    /// The signature check is the load-bearing one. This scan runs on items of
    /// every class, so an item whose last two bytes happen to look like an
    /// offset would otherwise produce a phantom block. A failed check means "no
    /// extension blocks", never an error.
    #[must_use]
    pub fn find(data: &'a [u8]) -> Option<Self> {
        let tail = data.get(data.len().checked_sub(2)?..)?;
        let from_item_start = usize::from(u16::from_le_bytes([tail[0], tail[1]]));
        // libfwsi requires >= 4 from the item start, i.e. past `cb` and the
        // class byte, and strictly before the trailing offset field itself.
        if from_item_start < 4 || from_item_start >= data.len() {
            return None;
        }
        Self::parse_at(data, from_item_start - MIN_ITEM_SIZE)
    }

    /// Every extension block on this item, in order.
    #[must_use]
    pub fn walk(data: &'a [u8]) -> ExtensionBlocks<'a> {
        ExtensionBlocks {
            data,
            next: Self::find(data).map(|b| b.offset),
        }
    }

    fn parse_at(data: &'a [u8], offset: usize) -> Option<Self> {
        let block = data.get(offset..)?;
        let size = usize::from(u16::from_le_bytes([*block.first()?, *block.get(1)?]));
        // A zero size is the "no more blocks" marker; anything below the
        // minimum cannot hold a header and would stall a walk.
        if size < MIN_EXTENSION_SIZE || size > block.len() {
            return None;
        }
        let version = u16::from_le_bytes([block[2], block[3]]);
        let signature = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        if signature & 0xFFFF_0000 != EXTENSION_SIGNATURE_PREFIX {
            return None;
        }
        Some(Self {
            offset,
            size,
            version,
            signature,
            body: &block[8..size],
        })
    }

    /// Decode as a `0xBEEF0004` file entry extension, or `None` for any other
    /// block.
    #[must_use]
    pub fn as_file_entry(self) -> Option<FileEntryExtension<'a>> {
        if self.signature != EXTENSION_FILE_ENTRY {
            return None;
        }
        FileEntryExtension::parse(self.version, self.body)
    }
}

/// Iterator over the extension blocks of a shell item.
///
/// Terminates on the first block that fails validation, and always advances by
/// the block's declared size, which [`ExtensionBlock::parse_at`] has already
/// proved to be at least [`MIN_EXTENSION_SIZE`] — so this cannot spin.
#[derive(Debug, Clone)]
pub struct ExtensionBlocks<'a> {
    data: &'a [u8],
    next: Option<usize>,
}

impl<'a> Iterator for ExtensionBlocks<'a> {
    type Item = ExtensionBlock<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let at = self.next?;
        let block = ExtensionBlock::parse_at(self.data, at);
        self.next = block.map(|b| b.offset + b.size);
        if block.is_none() {
            self.next = None;
        }
        block
    }
}

impl core::iter::FusedIterator for ExtensionBlocks<'_> {}

/// The `0xBEEF0004` block: creation and access times, and the long file name.
///
/// The layout grew across shell versions and the version field is the only way
/// to know which fields are present:
///
/// ```text
/// body[0..4]    FAT creation date/time
/// body[4..8]    FAT last access date/time
/// body[8..10]   u16 long name offset, from the start of the block; 0 = absent
/// -- version >= 7 --
/// body[10..12]  u16 unknown
/// body[12..20]  u64 NTFS file reference
/// body[20..28]  unknown
/// -- all versions --
/// body[10] or body[28]  u16 localized name offset; 0 = absent
/// -- version >= 9 --  4 bytes unknown
/// -- version >= 8 --  4 bytes unknown
/// ..            long name, UTF-16LE, NUL-terminated
/// ..            localized name; UTF-16LE from version 7, ANSI at version 3
/// ..            u16 offset of the first extension block
/// ```
///
/// The field at `body[8..10]` is a byte **offset**, not a length — a natural
/// misreading, and one that silently produces the wrong string on version 3
/// blocks where the value happens to be small. It is used here only as a
/// presence flag and a sanity check; the strings are read sequentially, which is
/// what libfwsi itself does.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct FileEntryExtension<'a> {
    pub version: u16,
    pub created: DosDateTime,
    pub accessed: DosDateTime,
    /// NTFS file reference: the low 48 bits are the MFT entry index and the top
    /// 16 the sequence number. Present from version `0x0007`.
    pub file_reference: Option<u64>,
    /// The long (non-8.3) name, UTF-16LE.
    pub long_name: Option<ShellStr<'a>>,
    /// The localized display name, when the folder has one.
    pub localized_name: Option<ShellStr<'a>>,
}

impl<'a> FileEntryExtension<'a> {
    /// MFT entry index out of [`FileEntryExtension::file_reference`].
    #[must_use]
    pub fn mft_entry(&self) -> Option<u64> {
        self.file_reference.map(|r| r & 0x0000_FFFF_FFFF_FFFF)
    }

    /// MFT sequence number out of [`FileEntryExtension::file_reference`].
    #[must_use]
    pub fn mft_sequence(&self) -> Option<u16> {
        self.file_reference.map(|r| (r >> 48) as u16)
    }

    fn parse(version: u16, body: &'a [u8]) -> Option<Self> {
        // libfwsi accepts only these four. An unrecognised version means the
        // offsets below are guesses, and a guessed offset produces a plausible
        // wrong filename, so decline instead.
        if !matches!(version, 3 | 7 | 8 | 9) {
            return None;
        }

        let created = DosDateTime::from_le_bytes(body.get(0..4)?.try_into().ok()?);
        let accessed = DosDateTime::from_le_bytes(body.get(4..8)?.try_into().ok()?);
        let long_name_offset = u16::from_le_bytes(body.get(8..10)?.try_into().ok()?);

        let mut pos = 10usize;
        let file_reference = if version >= 7 {
            // body[10..12] unknown, body[12..20] file reference, body[20..28] unknown
            let raw = u64::from_le_bytes(body.get(12..20)?.try_into().ok()?);
            pos = 28;
            Some(raw)
        } else {
            None
        };

        let localized_name_offset = u16::from_le_bytes(body.get(pos..pos + 2)?.try_into().ok()?);
        pos += 2;
        if version >= 9 {
            pos += 4;
        }
        if version >= 8 {
            pos += 4;
        }

        let long_name = if long_name_offset == 0 {
            None
        } else {
            let bytes = utf16_nul_terminated(body.get(pos..)?);
            pos += bytes.len() + 2;
            Some(ShellStr::Utf16(bytes))
        };

        let localized_name = if localized_name_offset == 0 {
            None
        } else {
            let rest = body.get(pos..)?;
            // Version 3 stored this as code page bytes; from version 7 it is
            // UTF-16LE like everything else.
            Some(if version >= 7 {
                ShellStr::Utf16(utf16_nul_terminated(rest))
            } else {
                ShellStr::Ansi(nul_terminated(rest))
            })
        };

        Some(Self {
            version,
            created,
            accessed,
            file_reference,
            long_name,
            localized_name,
        })
    }
}

/// Bytes up to the first NUL, or all of them if there is none.
///
/// A missing terminator is normal here: shell items pad fixed-width name fields
/// with zeros, and a name that exactly fills its field has no room for one.
fn nul_terminated(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

/// The same, for UTF-16LE: bytes up to the first `0x0000` unit on an even
/// boundary.
fn utf16_nul_terminated(bytes: &[u8]) -> &[u8] {
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == 0 && bytes[i + 1] == 0 {
            return &bytes[..i];
        }
        i += 2;
    }
    bytes
}

/// Step past a NUL-terminated string of `len` bytes, saturating at the end
/// rather than panicking when the terminator was missing.
fn advance_past_nul(bytes: &[u8], len: usize) -> &[u8] {
    let skip = (len + 1).min(bytes.len());
    &bytes[skip..]
}

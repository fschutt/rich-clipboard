//! Shell items recognised by a signature inside the body rather than by the
//! class byte.
//!
//! Several namespace extensions write items whose class type indicator is
//! `0x00` — a value that means nothing on its own — and identify themselves
//! with a 32-bit signature a few bytes in. A parser that dispatches on the
//! class byte alone sees `Unknown { class: 0x00 }` for all of them, which is
//! how an MTP device, a zip file's interior and the Users folder all end up
//! nameless in a breadcrumb.
//!
//! Everything here follows libyal's [Windows Shell Item format][libfwsi]
//! document *and* the `libfwsi` C parser. Where they disagree the code wins,
//! and each such case is called out at the field it affects.
//!
//! [libfwsi]: https://github.com/libyal/libfwsi/tree/main/documentation
//!
//! # Dispatch order, and why it is the order it is
//!
//! `libfwsi_item_copy_from_byte_stream` tries, in order: delegate folder (by a
//! class identifier near the *end* of the item), then signatures at three fixed
//! offsets, then the compressed folder character patterns, and only then the
//! class byte. [`recognise`] reproduces that order, because it is the order
//! that has been run against real captures.
//!
//! It is not a free lunch. The signature probes read fixed offsets on items of
//! every class, so a file entry whose `FileSize` or FAT timestamp happened to
//! equal a signature would be reclassified. The values involved make that a
//! curiosity rather than a risk — but it is the reason each probe here also
//! insists on a plausible length, and the reason nothing in this module can
//! fail: a wrong guess costs one breadcrumb segment, never the list.
//!
//! # Offsets in this file
//!
//! The forensics literature counts from the start of the *item*, including the
//! two bytes of `cb`. This module parses `abID`, which begins two bytes later,
//! so every documented offset appears here minus two. The doc comments give the
//! `abID`-relative numbers to match the code.

use crate::{guid::Guid, item::MIN_ITEM_SIZE, shell_item::ShellItem, string::ShellStr};

/// MTP storage device volume. At `abID[4..8]`.
pub const SIG_MTP_VOLUME: u32 = 0x1031_2005;
/// MTP storage device file entry. At `abID[4..8]`.
pub const SIG_MTP_FILE_ENTRY: u32 = 0x0719_2006;

/// The six data signatures a users property view item can carry, at
/// `abID[4..8]`.
///
/// Only `0x23FEBBEE` has a documented payload — a `KNOWNFOLDERID` — and it is
/// the one that matters, because that is the item Explorer writes for the Users
/// folder and for a library.
pub const SIG_USERS_PROPERTY_VIEW: [u32; 6] = [
    0x1014_1981,
    0x23A3_DFD5,
    0x23FE_BBEE,
    0x3B93_AFBB,
    // "ARPI"
    0x4950_5241,
    0xBEEB_EE00,
];

/// The users property view signature whose identifier is a `KNOWNFOLDERID`.
pub const SIG_USERS_PROPERTY_VIEW_KNOWN_FOLDER: u32 = 0x23FE_BBEE;

/// `{5E591A74-DF96-48D3-8D67-1733BCEE28BA}`, the class identifier that marks an
/// item as a delegate folder. Sits 32 bytes before the end of the item.
pub const DELEGATE_CLASS_ID: Guid = Guid::from_bytes([
    0x74, 0x1A, 0x59, 0x5E, 0x96, 0xDF, 0xD3, 0x48, 0x8D, 0x67, 0x17, 0x33, 0xBC, 0xEE, 0x28, 0xBA,
]);

/// Try every signature-based recognition, in libfwsi's order.
///
/// Returns `None` when nothing matches, which is the normal case and means
/// "fall through to the class byte".
///
/// `delegate` gates the delegate folder probe. It is `false` exactly once: when
/// interpreting the inside of a delegate folder, where allowing the probe again
/// would let a hostile PIDL nest wrappers and recurse.
#[must_use]
pub fn recognise(data: &[u8], delegate: bool) -> Option<ShellItem<'_>> {
    if delegate {
        if let Some(d) = DelegateFolder::parse(data) {
            return Some(ShellItem::DelegateFolder(d));
        }
    }
    match sig_at(data, 4) {
        Some(SIG_MTP_VOLUME) => {
            if let Some(v) = MtpVolume::parse(data) {
                return Some(ShellItem::MtpVolume(v));
            }
        }
        Some(SIG_MTP_FILE_ENTRY) => {
            if let Some(f) = MtpFileEntry::parse(data) {
                return Some(ShellItem::MtpFileEntry(f));
            }
        }
        Some(s) if SIG_USERS_PROPERTY_VIEW.contains(&s) => {
            if let Some(v) = UsersPropertyView::parse(data) {
                return Some(ShellItem::UsersPropertyView(v));
            }
        }
        _ => {}
    }
    CompressedFolder::parse(data).map(ShellItem::CompressedFolder)
}

/// A little-endian `u32` at an `abID` offset, or `None` if the item is shorter.
fn sig_at(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// A UTF-16LE string of `chars` code units at `at`, terminator included or not.
///
/// `chars` came off the wire, so the multiplication is checked and the slice is
/// a `get`. Returns `None` rather than a truncated string: half a filename is
/// not better than none.
fn utf16_chars(data: &[u8], at: usize, chars: usize) -> Option<ShellStr<'_>> {
    let bytes = chars.checked_mul(2)?;
    let end = at.checked_add(bytes)?;
    Some(ShellStr::Utf16(trim_utf16_nul(data.get(at..end)?)))
}

fn trim_utf16_nul(bytes: &[u8]) -> &[u8] {
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == 0 && bytes[i + 1] == 0 {
            return &bytes[..i];
        }
        i += 2;
    }
    bytes
}

/// A delegate folder item: one shell item wrapped in another, with a GUID
/// saying which extension owns the inner bytes.
///
/// ```text
/// abID[0]        class type indicator
/// abID[1]        unknown
/// abID[2..4]     u16 inner data size
/// abID[4..]      inner data, `inner data size` bytes
/// ..             trailing data
/// end-32..end-16 delegate class identifier, always DELEGATE_CLASS_ID
/// end-16..end    delegate folder identifier
/// ```
///
/// `end` is not simply the end of the item. The last two bytes of a shell item
/// hold the offset of the first extension block; when that offset is sane, the
/// two GUIDs sit before *it* rather than before the end of the item, because
/// the extension blocks come after them. libfwsi computes it exactly that way
/// and so does [`DelegateFolder::parse`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct DelegateFolder<'a> {
    pub class: u8,
    /// Which namespace extension owns [`DelegateFolder::inner`]. Try
    /// [`Guid::well_known_name`] — `Search Folder`, `Users Files delegate
    /// folder` and `Removable Drives` are all in the table.
    pub folder_id: Guid,
    /// The wrapped item's body, bounded by the declared inner data size.
    ///
    /// This is *usually* another `abID` and [`DelegateFolder::inner_item`]
    /// parses it as one, but not always: for four folder identifiers libfwsi
    /// skips four leading bytes first, and for the search folder it does not
    /// re-align at all.
    pub inner: &'a [u8],
    /// The entire body including the class byte.
    pub raw: &'a [u8],
}

impl<'a> DelegateFolder<'a> {
    /// Interpret [`DelegateFolder::inner`] as a shell item.
    ///
    /// Deliberately **not** recursive: the inner item is parsed by class byte
    /// and signature but is never itself unwrapped as a delegate folder. A
    /// PIDL that nested delegate folders a thousand deep would otherwise cost a
    /// thousand stack frames, and one level is all any real item uses.
    #[must_use]
    pub fn inner_item(&self) -> ShellItem<'a> {
        ShellItem::parse_no_delegate(self.inner)
    }

    fn parse(data: &'a [u8]) -> Option<Self> {
        // libfwsi requires an item of at least 38 bytes: two GUIDs, the six
        // byte header, and four bytes of something.
        let item_len = data.len().checked_add(MIN_ITEM_SIZE)?;
        if item_len < 38 {
            return None;
        }
        let tail = data.get(data.len() - 2..)?;
        let first_extension = usize::from(u16::from_le_bytes([tail[0], tail[1]]));

        // Item-relative length of the delegate folder region: the extension
        // block offset when it is sane, the whole item otherwise.
        let region = if first_extension >= 32 && first_extension < item_len - 2 {
            first_extension
        } else {
            item_len
        };
        if region < 38 {
            return None;
        }
        // Same value in `abID` coordinates.
        let end = region - MIN_ITEM_SIZE;
        if end > data.len() {
            return None;
        }

        let class_id = Guid::from_slice(data.get(end - 32..end - 16)?)?;
        if class_id != DELEGATE_CLASS_ID {
            return None;
        }
        let folder_id = Guid::from_slice(data.get(end - 16..end)?)?;

        let inner_size = usize::from(u16::from_le_bytes([*data.get(2)?, *data.get(3)?]));
        // libfwsi treats an oversized inner size as fatal. Here it is only a
        // reason to hand back an empty inner: the two GUIDs are the part worth
        // having and they have already been read.
        let inner = match region.checked_sub(38) {
            Some(max) if inner_size <= max => data.get(4..4 + inner_size).unwrap_or(&[]),
            _ => &[],
        };

        Some(Self {
            class: *data.first()?,
            folder_id,
            inner,
            raw: data,
        })
    }
}

/// An MTP (Media Transfer Protocol) storage device volume — a phone or camera
/// as it appears in `This PC`.
///
/// ```text
/// abID[0]       class type indicator, 0x00
/// abID[1]       unknown
/// abID[2..4]    u16 data size, extension blocks excluded
/// abID[4..8]    0x10312005
/// abID[36..40]  u32 name length, in characters, terminator included
/// abID[40..44]  u32 identifier length, likewise
/// abID[44..48]  u32 file system length, likewise
/// abID[48..52]  u32 number of GUID strings
/// abID[52..]    the three strings, then 78 bytes per GUID string
/// ```
///
/// Note the unit: these lengths count UTF-16 *characters*, not bytes. Reading
/// them as byte counts halves every string.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MtpVolume<'a> {
    pub class: u8,
    /// The volume's display name, e.g. `Internal storage`.
    pub name: Option<ShellStr<'a>>,
    /// The device-assigned storage identifier.
    pub identifier: Option<ShellStr<'a>>,
    /// The file system name the device reports.
    pub file_system: Option<ShellStr<'a>>,
    /// The entire body including the class byte.
    pub raw: &'a [u8],
}

impl<'a> MtpVolume<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        let name_chars = sig_at(data, 36)? as usize;
        let id_chars = sig_at(data, 40)? as usize;
        let fs_chars = sig_at(data, 44)? as usize;
        // 48..52 is the GUID string count; the strings after it are WPD event
        // handler identifiers and carry nothing a breadcrumb wants.

        let mut at = 52usize;
        let name = read_counted(data, &mut at, name_chars);
        let identifier = read_counted(data, &mut at, id_chars);
        let file_system = read_counted(data, &mut at, fs_chars);

        Some(Self {
            class: *data.first()?,
            name,
            identifier,
            file_system,
            raw: data,
        })
    }
}

/// Read a character-counted UTF-16LE string and advance past it.
///
/// A zero count means "absent", which is how libfwsi reads it: the field is
/// skipped entirely rather than contributing an empty string. A count that runs
/// past the item stops the sequence — every string after it would be read at a
/// wrong offset, and a wrong offset produces a plausible wrong name.
fn read_counted<'a>(data: &'a [u8], at: &mut usize, chars: usize) -> Option<ShellStr<'a>> {
    if chars == 0 {
        return None;
    }
    let s = utf16_chars(data, *at, chars)?;
    *at = at.checked_add(chars.checked_mul(2)?)?;
    Some(s)
}

/// A file or folder on an MTP storage device.
///
/// ```text
/// abID[0]       class type indicator, 0x00
/// abID[2..4]    u16 data size
/// abID[4..8]    0x07192006
/// abID[24..32]  FILETIME, modification time
/// abID[32..40]  FILETIME, creation time
/// abID[40..56]  content type GUID (`WPD_CONTENT_TYPE_FOLDER` for a folder)
/// abID[60..64]  u32 name length, in characters
/// abID[64..68]  u32 second name length
/// abID[68..72]  u32 identifier length
/// abID[72..]    the three strings
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MtpFileEntry<'a> {
    pub class: u8,
    /// 100-nanosecond intervals since 1601-01-01 UTC. Zero means unset.
    pub modified: u64,
    /// Likewise.
    pub created: u64,
    /// `WPD_CONTENT_TYPE_*`. Not resolved to a name here — the WPD content type
    /// GUIDs are a different namespace from the shell folder ones and mixing
    /// the tables would produce confident nonsense.
    pub content_type: Option<Guid>,
    /// The entry's name.
    pub name: Option<ShellStr<'a>>,
    /// A second name, usually identical to the first.
    pub name2: Option<ShellStr<'a>>,
    /// The device-assigned object identifier.
    pub identifier: Option<ShellStr<'a>>,
    /// The entire body including the class byte.
    pub raw: &'a [u8],
}

impl<'a> MtpFileEntry<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        let modified = read_u64(data, 24)?;
        let created = read_u64(data, 32)?;
        let content_type = data.get(40..56).and_then(Guid::from_slice);
        let name_chars = sig_at(data, 60)? as usize;
        let name2_chars = sig_at(data, 64)? as usize;
        let id_chars = sig_at(data, 68)? as usize;

        let mut at = 72usize;
        let name = read_counted(data, &mut at, name_chars);
        let name2 = read_counted(data, &mut at, name2_chars);
        let identifier = read_counted(data, &mut at, id_chars);

        Some(Self {
            class: *data.first()?,
            modified,
            created,
            content_type,
            name,
            name2,
            identifier,
            raw: data,
        })
    }
}

fn read_u64(data: &[u8], at: usize) -> Option<u64> {
    let b = data.get(at..at + 8)?;
    Some(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

/// The item Explorer writes below the Users folder and below a library.
///
/// ```text
/// abID[0]       class type indicator, 0x00
/// abID[1]       unknown
/// abID[2..4]    u16 data size, extension blocks excluded
/// abID[4..8]    u32 data signature
/// abID[8..10]   u16 property store size, 0 if absent
/// abID[10..12]  u16 identifier size
/// abID[12..]    identifier data
/// ..            property store, a serialized property storage
/// ```
///
/// The libfwsi *document* calls the field at `abID[2..4]` a 16-bit data size;
/// the *code* reads a 32-bit value there and assigns it to a 16-bit variable,
/// which truncates to the same answer on little-endian input. Same result,
/// different reasoning — recorded because the next field's offset depends on it
/// and the two would part company on a 32-bit size that overflowed.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct UsersPropertyView<'a> {
    pub class: u8,
    /// Which of [`SIG_USERS_PROPERTY_VIEW`] this item carries.
    pub signature: u32,
    /// The identifier blob, whose meaning depends on the signature.
    pub identifier: &'a [u8],
    /// The identifier decoded as a `KNOWNFOLDERID`, for the one signature that
    /// defines it that way ([`SIG_USERS_PROPERTY_VIEW_KNOWN_FOLDER`], with a
    /// 16-byte identifier). Try [`Guid::well_known_name`].
    pub known_folder_id: Option<Guid>,
    /// The serialized property storage, undecoded.
    ///
    /// These are [MS-PROPSTORE] bytes — byte-for-byte the same structure as a
    /// `.lnk` `PropertyStoreDataBlock` payload, which `rclip-shell-link`
    /// decodes. This crate keeps them raw rather than depending on that one:
    /// codec crates in this workspace do not depend on each other, and a PIDL
    /// parser has no business pulling in a shell link parser.
    ///
    /// [MS-PROPSTORE]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-propstore/39ea873f-7af5-44dd-92f9-bc1f293852cc
    pub property_store: &'a [u8],
    /// The entire body including the class byte.
    pub raw: &'a [u8],
}

impl<'a> UsersPropertyView<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        let signature = sig_at(data, 4)?;
        let store_size = usize::from(u16::from_le_bytes([*data.get(8)?, *data.get(9)?]));
        let id_size = usize::from(u16::from_le_bytes([*data.get(10)?, *data.get(11)?]));

        let identifier = data.get(12..12usize.checked_add(id_size)?).unwrap_or(&[]);
        let known_folder_id = if signature == SIG_USERS_PROPERTY_VIEW_KNOWN_FOLDER {
            Guid::from_slice(identifier)
        } else {
            None
        };
        let store_at = 12usize.checked_add(identifier.len())?;
        let property_store = data
            .get(store_at..store_at.checked_add(store_size)?)
            .unwrap_or(&[]);

        Some(Self {
            class: *data.first()?,
            signature,
            identifier,
            known_folder_id,
            property_store,
            raw: data,
        })
    }
}

/// Which shell wrote a compressed folder item. The layouts share nothing but
/// the idea.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum CompressedFolderVariant {
    /// Windows XP: the modification time is a 32-byte `"mm/dd/yy  HH:MM"`
    /// string and the name lengths sit at `abID[58]`.
    WindowsXp,
    /// Windows 10: sizes and a CRC-32 up front, and the name lengths at
    /// `abID[82]`.
    Windows10,
}

/// An entry *inside* a zip file, as the shell's compressed folder extension
/// names it.
///
/// This one is not identified by a signature at all — it is identified by
/// where the punctuation of a formatted date string lands. libfwsi checks for
/// `/`, `:` and spaces at fixed offsets inside the UTF-16 timestamp field, and
/// there is nothing better available: the item carries no magic number and its
/// class byte is one of `0x08`, `0x0B`, `0x12`, none of which mean anything.
///
/// The two layouts are in the module-level libfwsi document under "Compressed
/// folder shell item".
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CompressedFolder<'a> {
    /// The class type indicator. Not a discriminator here: `0x08`, `0x0B` and
    /// `0x12` have all been seen and none of them means anything.
    pub class: u8,
    pub variant: CompressedFolderVariant,
    /// Uncompressed size in bytes. Windows 10 layout only.
    pub uncompressed_size: Option<u64>,
    /// Compressed size in bytes. Windows 10 layout only.
    pub compressed_size: Option<u64>,
    /// The ZIP compression method: `0x00` stored, `0x08` DEFLATE. Windows 10
    /// layout only.
    pub compression_method: Option<u16>,
    /// CRC-32 of the uncompressed data, or zero when not available. Windows 10
    /// layout only.
    pub crc32: Option<u32>,
    /// The entry's path inside the archive.
    pub name: Option<ShellStr<'a>>,
    /// A second string, usually the archive-relative directory.
    pub name2: Option<ShellStr<'a>>,
    /// The entire body including the class byte.
    pub raw: &'a [u8],
}

impl<'a> CompressedFolder<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        let variant = detect(data)?;
        // Name lengths count characters and *exclude* the terminator — the
        // opposite of the MTP items above, which include it. The strings are
        // still NUL-terminated on the wire, so each one is `chars * 2 + 2`
        // bytes long.
        let (len_at, first_at) = match variant {
            CompressedFolderVariant::WindowsXp => (58usize, 66usize),
            CompressedFolderVariant::Windows10 => (82usize, 90usize),
        };
        let name_chars = sig_at(data, len_at).unwrap_or(0) as usize;
        let name2_chars = sig_at(data, len_at + 4).unwrap_or(0) as usize;

        let mut at = first_at;
        let name = read_terminated(data, &mut at, name_chars);
        let name2 = read_terminated(data, &mut at, name2_chars);

        let (uncompressed_size, compressed_size, compression_method, crc32) = match variant {
            CompressedFolderVariant::WindowsXp => (None, None, None, None),
            CompressedFolderVariant::Windows10 => (
                read_u64(data, 6),
                read_u64(data, 14),
                data.get(22..24).map(|b| u16::from_le_bytes([b[0], b[1]])),
                sig_at(data, 26),
            ),
        };

        Some(Self {
            class: *data.first()?,
            variant,
            uncompressed_size,
            compressed_size,
            compression_method,
            crc32,
            name,
            name2,
            raw: data,
        })
    }
}

/// The same as [`read_counted`], for a length that excludes the terminator.
fn read_terminated<'a>(data: &'a [u8], at: &mut usize, chars: usize) -> Option<ShellStr<'a>> {
    let s = utf16_chars(data, *at, chars)?;
    *at = at.checked_add(chars.checked_mul(2)?.checked_add(2)?)?;
    if s.is_empty() {
        return None;
    }
    Some(s)
}

/// `libfwsi_item_copy_from_byte_stream`'s three character-pattern tests, in its
/// order and at its offsets.
fn detect(data: &[u8]) -> Option<CompressedFolderVariant> {
    // Windows XP: "mm/dd/yy  HH:MM" as UTF-16LE at abID[22..], so the slashes
    // land at 26 and 32, the double space at 38 and 40, the colon at 46, and
    // the field ends with a NUL at 52.
    let xp = [
        (26usize, b'/'),
        (32, b'/'),
        (38, b' '),
        (40, b' '),
        (46, b':'),
        (52, 0),
    ];
    if data.len() >= 54 && xp.iter().all(|&(i, c)| ascii_utf16_at(data, i, c)) {
        return Some(CompressedFolderVariant::WindowsXp);
    }
    if data.len() < 76 {
        return None;
    }
    // Windows 10 with no timestamp: the literal "N/A" at abID[34..40].
    let na = [(34usize, b'N'), (36, b'/'), (38, b'A'), (40, 0)];
    if na.iter().all(|&(i, c)| ascii_utf16_at(data, i, c)) {
        return Some(CompressedFolderVariant::Windows10);
    }
    // Windows 10 with one: "mm/dd/yyyy  HH:MM:SS", four characters longer than
    // the XP form and starting six bytes later.
    let w10 = [
        (38usize, b'/'),
        (44, b'/'),
        (54, b' '),
        (56, b' '),
        (62, b':'),
        (68, b':'),
        (74, 0),
    ];
    if w10.iter().all(|&(i, c)| ascii_utf16_at(data, i, c)) {
        return Some(CompressedFolderVariant::Windows10);
    }
    None
}

/// `true` if `data` holds the UTF-16LE code unit for the ASCII byte `c` at
/// `at`.
fn ascii_utf16_at(data: &[u8], at: usize, c: u8) -> bool {
    matches!(data.get(at..at + 2), Some([lo, 0]) if *lo == c)
}

/// Class `0x71`: a Control Panel applet.
///
/// ```text
/// abID[0]       0x71
/// abID[1]       unknown, seen 0x80 (sort order?)
/// abID[2..12]   unknown, zero in every sample
/// abID[12..28]  control panel item identifier, a GUID
/// ```
///
/// 28 bytes of body, so `cb == 30`. The GUID is the whole item — there is no
/// name on the wire — which is why [`Guid::control_panel_name`] exists and why
/// [`ControlPanelItem::name`] is the only way this item ever gets one.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ControlPanelItem<'a> {
    pub class: u8,
    /// The applet's identifier.
    pub identifier: Guid,
    /// The entire body including the class byte.
    pub raw: &'a [u8],
}

impl<'a> ControlPanelItem<'a> {
    /// The applet's name, if it is one of the catalogued ones.
    #[must_use]
    pub fn name(&self) -> Option<&'static str> {
        self.identifier.control_panel_name()
    }

    pub(crate) fn parse(class: u8, data: &'a [u8]) -> Option<Self> {
        Some(Self {
            class,
            identifier: Guid::from_slice(data.get(12..28)?)?,
            raw: data,
        })
    }
}

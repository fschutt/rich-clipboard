//! TOC key numbers.
//!
//! These are not documented by Apple. Every constant here is cross-checked
//! against the three independent reverse-engineering efforts listed in the
//! crate README; where they disagree the disagreement is recorded in the doc
//! comment rather than silently resolved.
//!
//! A TOC entry key is *either* one of these numbers *or*, when bit 31 is set,
//! an offset to a string record naming the key. See [`crate::EntryKey`].

/// Bit 31 of a TOC entry key. When set, the low 31 bits are an offset to a
/// string record holding the key's name instead of being a number.
///
/// Easy to miss, and missing it turns a named key into a nonsense number in
/// the 2-billion range that then never matches anything.
pub const STRING_KEY_FLAG: u32 = 0x8000_0000;

/// Mask for the offset carried by a string key.
pub const STRING_KEY_MASK: u32 = 0x7FFF_FFFF;

/// Target URL, as a URL record. Absent from most modern bookmarks, which
/// describe the target with [`TARGET_PATH`] instead.
pub const TARGET_URL: u32 = 0x1003;
/// Array of the target's path components, root first, *without* separators.
pub const TARGET_PATH: u32 = 0x1004;
/// Array of the catalog node ID (inode) of each component of [`TARGET_PATH`].
pub const TARGET_CNID_PATH: u32 = 0x1005;
/// `CFURL` resource property flags: three little-endian `u64`s — the flags,
/// the mask of flags that were asked for, and eight reserved zero bytes.
/// See [`crate::flags`].
pub const TARGET_FLAGS: u32 = 0x1010;
/// The target's file name, as a string.
pub const TARGET_FILENAME: u32 = 0x1020;
/// The target's catalog node ID, as an integer.
pub const TARGET_CNID: u32 = 0x1030;
/// The target's creation date, as a date record.
pub const TARGET_CREATION_DATE: u32 = 0x1040;

/// Array of `(TOC id, ?)` pairs. `mac_alias` names this `kBookmarkTOCPath`;
/// the other two writeups do not mention it.
pub const TOC_PATH: u32 = 0x2000;
/// Path of the volume the target lives on, as a string.
pub const VOLUME_PATH: u32 = 0x2002;
/// The volume's URL, as a URL record.
pub const VOLUME_URL: u32 = 0x2005;
/// The volume's display name, as a string.
pub const VOLUME_NAME: u32 = 0x2010;
/// The volume's UUID — stored as a *string*, not as a `0x0801` UUID record.
/// Both `mac_alias` and Mother's Ruin call this out explicitly; do not assume
/// the record type from the key name.
pub const VOLUME_UUID: u32 = 0x2011;
/// The volume's total capacity in bytes, as an integer.
pub const VOLUME_SIZE: u32 = 0x2012;
/// The volume's creation date, as a date record.
pub const VOLUME_CREATION_DATE: u32 = 0x2013;
/// `CFURL` volume property flags, same three-`u64` shape as [`TARGET_FLAGS`].
pub const VOLUME_FLAGS: u32 = 0x2020;
/// True when the volume is the filesystem root.
pub const VOLUME_IS_ROOT: u32 = 0x2030;
/// Embedded bookmark for a disk image, given as a TOC identifier.
pub const VOLUME_BOOKMARK: u32 = 0x2040;
/// The volume's mount point, as a URL record.
pub const VOLUME_MOUNT_POINT: u32 = 0x2050;

/// Index into [`TARGET_PATH`] of the containing folder.
///
/// Mother's Ruin reads this as "number of path components below the user's
/// home directory" rather than an index into the path array. The number is the
/// same in the common case, so the two readings are hard to tell apart.
pub const CONTAINING_FOLDER: u32 = 0xC001;
/// Name of the user who created the bookmark (`CFCopyUserName()`).
pub const CREATOR_USERNAME: u32 = 0xC011;
/// Effective UID of the creating process.
pub const CREATOR_UID: u32 = 0xC012;
/// True when the URL the bookmark was made from was a file *reference* URL.
pub const WAS_FILE_REFERENCE: u32 = 0xD001;
/// The `NSURLBookmarkCreationOptions` the bookmark was created with.
pub const CREATION_OPTIONS: u32 = 0xD010;
/// Array of base-URL component lengths, present only when the bookmark was
/// made relative to a base URL.
pub const URL_LENGTHS: u32 = 0xE003;

/// Display name — the localised name Finder shows, which can differ from
/// [`TARGET_FILENAME`].
pub const DISPLAY_NAME: u32 = 0xF017;
/// Icon in `icns` format.
pub const ICON_DATA: u32 = 0xF020;
/// Icon reference / raw icon image data.
pub const ICON_REF: u32 = 0xF021;
/// Type binding information (`dnib` array).
pub const TYPE_BINDING_DATA: u32 = 0xF022;
/// When the bookmark itself was made, as a `float64` — note that this is a
/// plain number record, *not* a `0x0400` date record, so it is little-endian
/// where the date records are big-endian.
pub const CREATION_TIME: u32 = 0xF030;
/// Sandbox extension token granting read-write access to the target.
pub const SANDBOX_RW_EXTENSION: u32 = 0xF080;
/// Sandbox extension token granting read-only access to the target.
pub const SANDBOX_RO_EXTENSION: u32 = 0xF081;
/// Embedded legacy alias record, for bookmarks that carry both forms.
pub const ALIAS_DATA: u32 = 0xFE00;

/// Name for a known key, or `None` for one of the many still-unidentified
/// numbers (`0x1054`, `0x1101`, `0x2070`, …).
///
/// Intended for dumping tools and test failure messages; parsing never depends
/// on a key being recognised.
#[must_use]
pub const fn name(key: u32) -> Option<&'static str> {
    Some(match key {
        TARGET_URL => "target URL",
        TARGET_PATH => "target path",
        TARGET_CNID_PATH => "target CNID path",
        TARGET_FLAGS => "target flags",
        TARGET_FILENAME => "target filename",
        TARGET_CNID => "target CNID",
        TARGET_CREATION_DATE => "target creation date",
        TOC_PATH => "TOC path",
        VOLUME_PATH => "volume path",
        VOLUME_URL => "volume URL",
        VOLUME_NAME => "volume name",
        VOLUME_UUID => "volume UUID",
        VOLUME_SIZE => "volume size",
        VOLUME_CREATION_DATE => "volume creation date",
        VOLUME_FLAGS => "volume flags",
        VOLUME_IS_ROOT => "volume is root",
        VOLUME_BOOKMARK => "volume bookmark",
        VOLUME_MOUNT_POINT => "volume mount point",
        CONTAINING_FOLDER => "containing folder index",
        CREATOR_USERNAME => "creator username",
        CREATOR_UID => "creator UID",
        WAS_FILE_REFERENCE => "was file reference",
        CREATION_OPTIONS => "creation options",
        URL_LENGTHS => "URL lengths",
        DISPLAY_NAME => "display name",
        ICON_DATA => "icon data",
        ICON_REF => "icon ref",
        TYPE_BINDING_DATA => "type binding data",
        CREATION_TIME => "bookmark creation time",
        SANDBOX_RW_EXTENSION => "sandbox read-write extension",
        SANDBOX_RO_EXTENSION => "sandbox read-only extension",
        ALIAS_DATA => "alias data",
        _ => return None,
    })
}

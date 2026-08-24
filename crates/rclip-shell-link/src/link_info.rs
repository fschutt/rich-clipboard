//! MS-SHLLINK 2.3 — `LinkInfo`, and the `VolumeID` and
//! `CommonNetworkRelativeLink` inside it.
//!
//! This is the fallback the shell uses when the target IDList will not bind:
//! the drive serial number, the volume label, the drive-letter path, and a UNC
//! form if there was one.
//!
//! # Everything in here is an offset
//!
//! Every offset field in `LinkInfo` is relative to the **start of the `LinkInfo`
//! structure**, and every offset in `VolumeID` and `CommonNetworkRelativeLink`
//! is relative to the start of *that* structure — not to the file, and not to
//! each other. Getting the base wrong produces strings from the middle of an
//! unrelated field rather than an error, which is why each of these types owns
//! the exact slice its own offsets are measured from.

use core::fmt;

use rclip_core::{Error, ErrorKind, Reader, Result};
use rclip_idlist::ShellStr;

/// Smallest legal `LinkInfoHeaderSize`: the seven mandatory `u32` header fields.
pub const LINK_INFO_HEADER_MIN: u32 = 0x0000_001C;
/// `LinkInfoHeaderSize` at or above this means the two Unicode offset fields are
/// present. Note it is `>=`, not `==`: a larger header is legal and the variable
/// data simply starts further in.
pub const LINK_INFO_HEADER_WITH_UNICODE: u32 = 0x0000_0024;

/// MS-SHLLINK 2.3 — how to find the target if the IDList does not resolve.
#[derive(Debug, Clone)]
pub struct LinkInfo<'a> {
    /// Exactly `LinkInfoSize` bytes, starting at the `LinkInfoSize` field.
    /// Every offset below indexes into this.
    buf: &'a [u8],
    /// `LinkInfoSize`. All offsets in the structure MUST be less than this.
    pub size: u32,
    /// `LinkInfoHeaderSize`, which decides whether the Unicode offsets exist.
    pub header_size: u32,
    pub flags: LinkInfoFlags,
    /// Offset of the `VolumeID`, or zero when `VolumeIDAndLocalBasePath` is
    /// clear.
    pub volume_id_offset: u32,
    pub local_base_path_offset: u32,
    pub common_network_relative_link_offset: u32,
    /// Not gated by any flag — the common path suffix is always present, though
    /// it is usually the empty string.
    pub common_path_suffix_offset: u32,
    /// Present only when `header_size >= 0x24`.
    pub local_base_path_offset_unicode: Option<u32>,
    /// Present only when `header_size >= 0x24`.
    pub common_path_suffix_offset_unicode: Option<u32>,
}

impl<'a> LinkInfo<'a> {
    /// Read from the cursor, advancing it past the whole structure.
    pub(crate) fn parse(r: &mut Reader<'a>) -> Result<Self> {
        let at = r.pos();
        let size = r.peek_u32_le()?;
        let size_usize = usize::try_from(size).map_err(|_| Error::new(ErrorKind::TooLarge, at))?;
        if size_usize < LINK_INFO_HEADER_MIN as usize {
            // Too small to hold even the mandatory fields, so nothing in it can
            // be trusted; and taking a sub-reader of it would leave the outer
            // cursor pointing into the middle of a structure.
            return Err(Error::new(ErrorKind::BadLength, at));
        }
        // A sub-reader bounded by LinkInfoSize: an offset field inside cannot
        // reach past the structure it belongs to, whatever it claims.
        let inner = r.take_reader(size_usize)?;
        let buf = inner.buffer();

        let mut h = Reader::new(buf);
        let size = h.u32_le()?;
        let header_size = h.u32_le()?;
        if header_size < LINK_INFO_HEADER_MIN || header_size as usize > buf.len() {
            return Err(Error::new(ErrorKind::BadLength, at + 4));
        }
        let flags = LinkInfoFlags(h.u32_le()?);
        let volume_id_offset = h.u32_le()?;
        let local_base_path_offset = h.u32_le()?;
        let common_network_relative_link_offset = h.u32_le()?;
        let common_path_suffix_offset = h.u32_le()?;

        let (local_base_path_offset_unicode, common_path_suffix_offset_unicode) =
            if header_size >= LINK_INFO_HEADER_WITH_UNICODE {
                (Some(h.u32_le()?), Some(h.u32_le()?))
            } else {
                (None, None)
            };

        Ok(Self {
            buf,
            size,
            header_size,
            flags,
            volume_id_offset,
            local_base_path_offset,
            common_network_relative_link_offset,
            common_path_suffix_offset,
            local_base_path_offset_unicode,
            common_path_suffix_offset_unicode,
        })
    }

    /// The whole `LinkInfo` structure, for re-serialization.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.buf
    }

    /// The volume the target was on, if `VolumeIDAndLocalBasePath` is set.
    pub fn volume_id(&self) -> Result<Option<VolumeId<'a>>> {
        if !self.flags.contains(LinkInfoFlags::VOLUME_ID_AND_LOCAL_BASE_PATH) {
            return Ok(None);
        }
        VolumeId::parse(self.buf, offset_of(self.volume_id_offset)?).map(Some)
    }

    /// The drive-letter path to the target, if `VolumeIDAndLocalBasePath` is
    /// set.
    ///
    /// Prefers the Unicode field when the header carries one and its offset is
    /// non-zero: MS-SHLLINK says Windows writes the Unicode form precisely when
    /// the ANSI form would have been truncated, so the ANSI field is the lossy
    /// one whenever both exist.
    pub fn local_base_path(&self) -> Result<Option<ShellStr<'a>>> {
        if !self.flags.contains(LinkInfoFlags::VOLUME_ID_AND_LOCAL_BASE_PATH) {
            return Ok(None);
        }
        if let Some(off) = self.local_base_path_offset_unicode.filter(|o| *o != 0) {
            return utf16_at(self.buf, offset_of(off)?).map(Some);
        }
        ansi_at(self.buf, offset_of(self.local_base_path_offset)?).map(Some)
    }

    /// The suffix appended to the local base path (or to the network link's
    /// net name) to form the full path. Usually empty.
    pub fn common_path_suffix(&self) -> Result<ShellStr<'a>> {
        if let Some(off) = self.common_path_suffix_offset_unicode.filter(|o| *o != 0) {
            return utf16_at(self.buf, offset_of(off)?);
        }
        ansi_at(self.buf, offset_of(self.common_path_suffix_offset)?)
    }

    /// The network location the target was on, if
    /// `CommonNetworkRelativeLinkAndPathSuffix` is set.
    pub fn common_network_relative_link(&self) -> Result<Option<CommonNetworkRelativeLink<'a>>> {
        if !self.flags.contains(LinkInfoFlags::COMMON_NETWORK_RELATIVE_LINK_AND_PATH_SUFFIX) {
            return Ok(None);
        }
        CommonNetworkRelativeLink::parse(
            self.buf,
            offset_of(self.common_network_relative_link_offset)?,
        )
        .map(Some)
    }
}

/// MS-SHLLINK 2.3 `LinkInfoFlags`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct LinkInfoFlags(pub u32);

impl LinkInfoFlags {
    /// `VolumeID` and `LocalBasePath` are present.
    pub const VOLUME_ID_AND_LOCAL_BASE_PATH: Self = Self(0x0000_0001);
    /// `CommonNetworkRelativeLink` is present.
    pub const COMMON_NETWORK_RELATIVE_LINK_AND_PATH_SUFFIX: Self = Self(0x0000_0002);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl fmt::Debug for LinkInfoFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LinkInfoFlags({:#010X}", self.0)?;
        if self.contains(Self::VOLUME_ID_AND_LOCAL_BASE_PATH) {
            f.write_str(": VolumeIDAndLocalBasePath")?;
        }
        if self.contains(Self::COMMON_NETWORK_RELATIVE_LINK_AND_PATH_SUFFIX) {
            f.write_str(" | CommonNetworkRelativeLinkAndPathSuffix")?;
        }
        f.write_str(")")
    }
}

/// MS-SHLLINK 2.3.1 — the volume the link target lived on.
#[derive(Debug, Clone)]
pub struct VolumeId<'a> {
    /// Exactly `VolumeIDSize` bytes. The label offsets index into this.
    buf: &'a [u8],
    /// `VolumeIDSize`, which the spec requires to be strictly greater than
    /// `0x10` — the minimum legal value is `0x11`.
    pub size: u32,
    pub drive_type: DriveType,
    /// The volume serial number, which is what link tracking actually matches
    /// on when a drive letter has changed.
    pub drive_serial_number: u32,
    pub volume_label_offset: u32,
    /// Present only when `volume_label_offset == 0x14`, which is the spec's
    /// sentinel for "the label is Unicode, look over there instead".
    pub volume_label_offset_unicode: Option<u32>,
}

impl<'a> VolumeId<'a> {
    fn parse(parent: &'a [u8], at: usize) -> Result<Self> {
        let head = Reader::new(parent).tail_at(at)?;
        let mut r = Reader::new(head);
        let size = r.u32_le()?;
        let size_usize = usize::try_from(size).map_err(|_| Error::new(ErrorKind::TooLarge, at))?;
        // "MUST be greater than 0x00000010" — strictly greater, so 0x11 is the
        // floor and 0x10 is malformed rather than merely empty.
        if size_usize <= 0x10 || size_usize > head.len() {
            return Err(Error::new(ErrorKind::BadLength, at));
        }
        let buf = &head[..size_usize];

        let mut r = Reader::new(buf);
        let size = r.u32_le()?;
        let drive_type = DriveType(r.u32_le()?);
        let drive_serial_number = r.u32_le()?;
        let volume_label_offset = r.u32_le()?;
        let volume_label_offset_unicode =
            if volume_label_offset == 0x0000_0014 { Some(r.u32_le()?) } else { None };

        Ok(Self {
            buf,
            size,
            drive_type,
            drive_serial_number,
            volume_label_offset,
            volume_label_offset_unicode,
        })
    }

    /// The volume label.
    ///
    /// `VolumeLabelOffset == 0x14` is a sentinel, not an offset: it means "this
    /// field MUST be ignored, use `VolumeLabelOffsetUnicode`". Following it as
    /// an offset lands exactly on the Unicode offset field and yields four bytes
    /// of garbage that look like a two-character label.
    pub fn volume_label(&self) -> Result<ShellStr<'a>> {
        match self.volume_label_offset_unicode {
            Some(off) => utf16_at(self.buf, offset_of(off)?),
            None => ansi_at(self.buf, offset_of(self.volume_label_offset)?),
        }
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.buf
    }
}

/// MS-SHLLINK 2.3.1 `DriveType`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct DriveType(pub u32);

impl DriveType {
    pub const UNKNOWN: Self = Self(0);
    pub const NO_ROOT_DIR: Self = Self(1);
    pub const REMOVABLE: Self = Self(2);
    pub const FIXED: Self = Self(3);
    pub const REMOTE: Self = Self(4);
    pub const CDROM: Self = Self(5);
    pub const RAMDISK: Self = Self(6);

    /// The `DRIVE_*` name, or `None` for a value outside the defined set.
    #[must_use]
    pub const fn name(self) -> Option<&'static str> {
        Some(match self.0 {
            0 => "DRIVE_UNKNOWN",
            1 => "DRIVE_NO_ROOT_DIR",
            2 => "DRIVE_REMOVABLE",
            3 => "DRIVE_FIXED",
            4 => "DRIVE_REMOTE",
            5 => "DRIVE_CDROM",
            6 => "DRIVE_RAMDISK",
            _ => return None,
        })
    }
}

impl fmt::Debug for DriveType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(n) => write!(f, "DriveType({}: {n})", self.0),
            None => write!(f, "DriveType({}: undefined)", self.0),
        }
    }
}

/// MS-SHLLINK 2.3.2 — the UNC path and mapped drive the target was reached
/// through.
#[derive(Debug, Clone)]
pub struct CommonNetworkRelativeLink<'a> {
    buf: &'a [u8],
    /// MUST be at least `0x14`.
    pub size: u32,
    pub flags: CommonNetworkRelativeLinkFlags,
    pub net_name_offset: u32,
    /// Zero unless `ValidDevice` is set.
    pub device_name_offset: u32,
    /// Only meaningful when `ValidNetType` is set.
    pub network_provider_type: NetworkProviderType,
    pub net_name_offset_unicode: Option<u32>,
    pub device_name_offset_unicode: Option<u32>,
}

impl<'a> CommonNetworkRelativeLink<'a> {
    fn parse(parent: &'a [u8], at: usize) -> Result<Self> {
        let head = Reader::new(parent).tail_at(at)?;
        let mut r = Reader::new(head);
        let size = r.u32_le()?;
        let size_usize = usize::try_from(size).map_err(|_| Error::new(ErrorKind::TooLarge, at))?;
        if size_usize < 0x14 || size_usize > head.len() {
            return Err(Error::new(ErrorKind::BadLength, at));
        }
        let buf = &head[..size_usize];

        let mut r = Reader::new(buf);
        let size = r.u32_le()?;
        let flags = CommonNetworkRelativeLinkFlags(r.u32_le()?);
        let net_name_offset = r.u32_le()?;
        let device_name_offset = r.u32_le()?;
        let network_provider_type = NetworkProviderType(r.u32_le()?);

        // Both Unicode offset fields live at fixed positions 0x14 and 0x18, so
        // they are either both present or both absent — and they can only be
        // present when NetName starts later than 0x14, because otherwise the
        // string data occupies those bytes.
        //
        // MS-SHLLINK words this asymmetrically: it gates NetNameOffsetUnicode on
        // `NetNameOffset > 0x14` but DeviceNameOffsetUnicode on
        // `DeviceNameOffset > 0x14`, which cannot both be honoured — with
        // ValidDevice clear, DeviceNameOffset MUST be zero, and then the
        // structure would have a hole where DeviceNameOffsetUnicode belongs.
        // Every implementation, and the layout itself, uses NetNameOffset for
        // both.
        let (net_name_offset_unicode, device_name_offset_unicode) =
            if net_name_offset > 0x0000_0014 {
                (Some(r.u32_le()?), Some(r.u32_le()?))
            } else {
                (None, None)
            };

        Ok(Self {
            buf,
            size,
            flags,
            net_name_offset,
            device_name_offset,
            network_provider_type,
            net_name_offset_unicode,
            device_name_offset_unicode,
        })
    }

    /// The server share path, e.g. `\\server\share`.
    pub fn net_name(&self) -> Result<ShellStr<'a>> {
        match self.net_name_offset_unicode.filter(|o| *o != 0) {
            Some(off) => utf16_at(self.buf, offset_of(off)?),
            None => ansi_at(self.buf, offset_of(self.net_name_offset)?),
        }
    }

    /// The mapped drive letter, e.g. `D:`. Absent unless `ValidDevice` is set.
    pub fn device_name(&self) -> Result<Option<ShellStr<'a>>> {
        if !self.flags.contains(CommonNetworkRelativeLinkFlags::VALID_DEVICE) {
            return Ok(None);
        }
        match self.device_name_offset_unicode.filter(|o| *o != 0) {
            Some(off) => utf16_at(self.buf, offset_of(off)?).map(Some),
            None => ansi_at(self.buf, offset_of(self.device_name_offset)?).map(Some),
        }
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.buf
    }
}

/// MS-SHLLINK 2.3.2 `CommonNetworkRelativeLinkFlags`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct CommonNetworkRelativeLinkFlags(pub u32);

impl CommonNetworkRelativeLinkFlags {
    /// `DeviceNameOffset` holds a real offset; otherwise it MUST be zero.
    pub const VALID_DEVICE: Self = Self(0x0000_0001);
    /// `NetworkProviderType` is meaningful; otherwise it MUST be ignored.
    pub const VALID_NET_TYPE: Self = Self(0x0000_0002);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl fmt::Debug for CommonNetworkRelativeLinkFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CommonNetworkRelativeLinkFlags({:#010X}", self.0)?;
        if self.contains(Self::VALID_DEVICE) {
            f.write_str(": ValidDevice")?;
        }
        if self.contains(Self::VALID_NET_TYPE) {
            f.write_str(" | ValidNetType")?;
        }
        f.write_str(")")
    }
}

/// MS-SHLLINK 2.3.2 `NetworkProviderType` — a `WNNC_NET_*` value.
///
/// Only meaningful when `ValidNetType` is set.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct NetworkProviderType(pub u32);

impl NetworkProviderType {
    /// The `WNNC_NET_*` name, or `None` if the value is not in the spec's
    /// table.
    ///
    /// The table runs `0x001A0000` to `0x00430000` in steps of `0x00010000`
    /// with exactly one gap: `0x00280000` is absent. That is the spec's table
    /// and not a transcription slip.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        let index = self.0.checked_sub(0x001A_0000)? >> 16;
        WNNC_NET_NAMES.get(index as usize).copied().filter(|n| !n.is_empty())
    }
}

impl fmt::Debug for NetworkProviderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(n) => write!(f, "NetworkProviderType({:#010X}: WNNC_NET_{n})", self.0),
            None => write!(f, "NetworkProviderType({:#010X})", self.0),
        }
    }
}

/// Indexed by `(value - 0x001A0000) >> 16`. The empty string at index 14 is the
/// `0x00280000` gap.
static WNNC_NET_NAMES: &[&str] = &[
    "AVID",         // 0x001A0000
    "DOCUSPACE",    // 0x001B0000
    "MANGOSOFT",    // 0x001C0000
    "SERNET",       // 0x001D0000
    "RIVERFRONT1",  // 0x001E0000
    "RIVERFRONT2",  // 0x001F0000
    "DECORB",       // 0x00200000
    "PROTSTOR",     // 0x00210000
    "FJ_REDIR",     // 0x00220000
    "DISTINCT",     // 0x00230000
    "TWINS",        // 0x00240000
    "RDR2SAMPLE",   // 0x00250000
    "CSC",          // 0x00260000
    "3IN1",         // 0x00270000
    "",             // 0x00280000 — unassigned in MS-SHLLINK
    "EXTENDNET",    // 0x00290000
    "STAC",         // 0x002A0000
    "FOXBAT",       // 0x002B0000
    "YAHOO",        // 0x002C0000
    "EXIFS",        // 0x002D0000
    "DAV",          // 0x002E0000
    "KNOWARE",      // 0x002F0000
    "OBJECT_DIRE",  // 0x00300000
    "MASFAX",       // 0x00310000
    "HOB_NFS",      // 0x00320000
    "SHIVA",        // 0x00330000
    "IBMAL",        // 0x00340000
    "LOCK",         // 0x00350000
    "TERMSRV",      // 0x00360000
    "SRT",          // 0x00370000
    "QUINCY",       // 0x00380000
    "OPENAFS",      // 0x00390000
    "AVID1",        // 0x003A0000
    "DFS",          // 0x003B0000
    "KWNP",         // 0x003C0000
    "ZENWORKS",     // 0x003D0000
    "DRIVEONWEB",   // 0x003E0000
    "VMWARE",       // 0x003F0000
    "RSFX",         // 0x00400000
    "MFILES",       // 0x00410000
    "MS_NFS",       // 0x00420000
    "GOOGLE",       // 0x00430000
];

/// Convert a wire offset to an index, failing rather than truncating on a
/// 16-bit `usize`.
fn offset_of(raw: u32) -> Result<usize> {
    usize::try_from(raw).map_err(|_| Error::new(ErrorKind::TooLarge, 0))
}

/// A NUL-terminated system-code-page string at an offset inside `buf`.
fn ansi_at(buf: &[u8], at: usize) -> Result<ShellStr<'_>> {
    let mut r = Reader::new(buf);
    r.seek(at)?;
    r.cstr_bytes().map(ShellStr::Ansi)
}

/// A NUL-terminated UTF-16LE string at an offset inside `buf`.
fn utf16_at(buf: &[u8], at: usize) -> Result<ShellStr<'_>> {
    let mut r = Reader::new(buf);
    r.seek(at)?;
    r.utf16_nul_bytes().map(ShellStr::Utf16)
}

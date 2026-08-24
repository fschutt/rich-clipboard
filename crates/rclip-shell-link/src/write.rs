//! Writing a shell link. Requires `alloc`.
//!
//! The seeding template had no writer, and reading is only half of what a
//! clipboard library needs: dragging a shortcut *out* of an application into
//! Explorer means producing a `.lnk`, and so does offering `CFSTR_SHELLLINK` on
//! the clipboard. See `plan/PLAN.md` §4.8.
//!
//! # What makes a link resolvable
//!
//! A shell link can name its target three ways, and Explorer tries them in
//! order: the target `LinkTargetIDList`, the `LinkInfo` local base path, and the
//! `EnvironmentVariableDataBlock`. The builder can write all three, but only the
//! last two from a plain string — see [`ShellLinkBuilder::target_id_list`] for
//! why an IDList has to be copied rather than synthesised.
//!
//! # Encoding
//!
//! `StringData` is always written as UTF-16LE with `IsUnicode` set. The
//! alternative is the writer's system default code page, which is not recorded
//! anywhere in the file, so an ANSI-only link is only reliably readable on a
//! machine configured like the one that wrote it. Where the format *forces* an
//! ANSI field — `LinkInfo`'s local base path, the fixed 260-byte half of an
//! environment variable block — non-ASCII characters are written as `?` and the
//! Unicode companion field carries the real text, which is what Windows itself
//! does.

extern crate alloc;

use alloc::{string::String, vec, vec::Vec};

use rclip_core::{Error, ErrorKind, Result};

use crate::{
    extra::{
        MIN_BLOCK_SIZE, SIG_ENVIRONMENT_VARIABLE, SIG_ICON_ENVIRONMENT, SIZE_ENVIRONMENT_VARIABLE,
    },
    filetime::FileTime,
    header::{FileAttributes, HotKey, LinkFlags, ShellLinkHeader, ShowCommand},
    link_info::{DriveType, LinkInfoFlags, LINK_INFO_HEADER_MIN, LINK_INFO_HEADER_WITH_UNICODE},
    string_data::check_writable_length,
};

/// The volume half of a [`ShellLinkBuilder`]'s `LinkInfo`.
#[derive(Debug, Clone, Default)]
struct Volume {
    drive_type: DriveType,
    serial_number: u32,
    label: String,
}

/// Builds the bytes of a `.lnk`.
///
/// ```
/// # fn main() -> Result<(), rclip_core::Error> {
/// use rclip_shell_link::{ShellLink, ShellLinkBuilder};
///
/// let bytes = ShellLinkBuilder::new()
///     .name("Notes")
///     .local_path(r"C:\Users\me\notes.txt")
///     .working_dir(r"C:\Users\me")
///     .build()?;
///
/// let link = ShellLink::parse(&bytes)?;
/// assert_eq!(link.string_data.name.unwrap().to_string_lossy(), "Notes");
/// let info = link.link_info.unwrap();
/// assert_eq!(
///     info.local_base_path()?.unwrap().to_string_lossy(),
///     r"C:\Users\me\notes.txt",
/// );
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ShellLinkBuilder {
    file_attributes: FileAttributes,
    creation_time: FileTime,
    access_time: FileTime,
    write_time: FileTime,
    file_size: u32,
    icon_index: i32,
    show_command: Option<ShowCommand>,
    hot_key: HotKey,

    target_id_list: Option<Vec<u8>>,
    local_path: Option<String>,
    volume: Option<Volume>,

    name: Option<String>,
    relative_path: Option<String>,
    working_dir: Option<String>,
    arguments: Option<String>,
    icon_location: Option<String>,

    environment_path: Option<String>,
    icon_environment_path: Option<String>,
    extra_blocks: Vec<Vec<u8>>,
}

impl ShellLinkBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a target `ITEMIDLIST`, as raw bytes.
    ///
    /// **Copy an IDList you were given** — from a `CFSTR_SHELLIDLIST` you
    /// received, or from `SHGetIDListFromObject` — rather than building one.
    /// The shell binds a PIDL by handing the bytes back to the namespace
    /// extension that owns them, so a hand-forged file entry either resolves to
    /// something unintended or does not resolve at all. `rclip_idlist`'s builder
    /// exists for the same reason and with the same caveat.
    ///
    /// The two-byte terminator is appended if the supplied bytes do not already
    /// end with one, because a `LinkTargetIDList` without it is malformed and
    /// the mistake is invisible until something tries to read the link.
    #[must_use]
    pub fn target_id_list(mut self, bytes: &[u8]) -> Self {
        let mut v = bytes.to_vec();
        if v.len() < 2 || v[v.len() - 2..] != [0, 0] {
            v.extend_from_slice(&[0, 0]);
        }
        self.target_id_list = Some(v);
        self
    }

    /// Write a `LinkInfo` naming a drive-letter path, e.g.
    /// `C:\Users\me\notes.txt`.
    ///
    /// This is the fallback Explorer uses when the target IDList will not bind,
    /// and on its own it is enough for a link that opens the right file on the
    /// machine that made it.
    #[must_use]
    pub fn local_path(mut self, path: &str) -> Self {
        self.local_path = Some(String::from(path));
        self
    }

    /// Record the volume the local path is on. Only meaningful alongside
    /// [`ShellLinkBuilder::local_path`].
    ///
    /// The serial number is what link tracking matches on when a drive letter
    /// has changed, so a real one is worth passing if you have it.
    #[must_use]
    pub fn volume(mut self, drive_type: DriveType, serial_number: u32, label: &str) -> Self {
        self.volume = Some(Volume {
            drive_type,
            serial_number,
            label: String::from(label),
        });
        self
    }

    /// `NAME_STRING`: the description shown to the user.
    #[must_use]
    pub fn name(mut self, s: &str) -> Self {
        self.name = Some(String::from(s));
        self
    }

    /// `RELATIVE_PATH`: the target's location relative to the `.lnk` itself.
    #[must_use]
    pub fn relative_path(mut self, s: &str) -> Self {
        self.relative_path = Some(String::from(s));
        self
    }

    /// `WORKING_DIR`.
    #[must_use]
    pub fn working_dir(mut self, s: &str) -> Self {
        self.working_dir = Some(String::from(s));
        self
    }

    /// `COMMAND_LINE_ARGUMENTS`. The one `StringData` field the spec does not
    /// cap at 260 characters.
    #[must_use]
    pub fn arguments(mut self, s: &str) -> Self {
        self.arguments = Some(String::from(s));
        self
    }

    /// `ICON_LOCATION`, paired with [`ShellLinkBuilder::icon_index`].
    #[must_use]
    pub fn icon_location(mut self, s: &str) -> Self {
        self.icon_location = Some(String::from(s));
        self
    }

    /// Index into the icon location. Negative values are resource IDs.
    #[must_use]
    pub const fn icon_index(mut self, index: i32) -> Self {
        self.icon_index = index;
        self
    }

    /// An `EnvironmentVariableDataBlock` target path, e.g.
    /// `%windir%\system32\cmd.exe`.
    ///
    /// The most portable way to name a target: it survives being opened on a
    /// machine whose drive layout differs, and it needs no IDList.
    #[must_use]
    pub fn environment_path(mut self, s: &str) -> Self {
        self.environment_path = Some(String::from(s));
        self
    }

    /// An `IconEnvironmentDataBlock` path — the icon's location written with
    /// environment variables.
    #[must_use]
    pub fn icon_environment_path(mut self, s: &str) -> Self {
        self.icon_environment_path = Some(String::from(s));
        self
    }

    #[must_use]
    pub const fn show_command(mut self, cmd: ShowCommand) -> Self {
        self.show_command = Some(cmd);
        self
    }

    #[must_use]
    pub const fn hot_key(mut self, hot_key: HotKey) -> Self {
        self.hot_key = hot_key;
        self
    }

    #[must_use]
    pub const fn file_attributes(mut self, attrs: FileAttributes) -> Self {
        self.file_attributes = attrs;
        self
    }

    /// Low 32 bits of the target's size, as recorded in the header.
    #[must_use]
    pub const fn file_size(mut self, size: u32) -> Self {
        self.file_size = size;
        self
    }

    /// The three header `FILETIME`s. Zero — the default — means "not recorded",
    /// which is legal and common.
    #[must_use]
    pub const fn times(mut self, creation: FileTime, access: FileTime, write: FileTime) -> Self {
        self.creation_time = creation;
        self.access_time = access;
        self.write_time = write;
        self
    }

    /// Append a raw `ExtraData` block: `body` goes after the size and signature,
    /// both of which are written for you.
    ///
    /// The escape hatch for the blocks this builder has no typed setter for. The
    /// caller is responsible for the body matching what the signature implies —
    /// including any fixed size MS-SHLLINK 2.5 requires, which the reader in
    /// this crate does enforce.
    #[must_use]
    pub fn extra_block(mut self, signature: u32, body: &[u8]) -> Self {
        let size = (body.len() + MIN_BLOCK_SIZE as usize) as u32;
        let mut block = Vec::with_capacity(body.len() + 8);
        block.extend_from_slice(&size.to_le_bytes());
        block.extend_from_slice(&signature.to_le_bytes());
        block.extend_from_slice(body);
        self.extra_blocks.push(block);
        self
    }

    /// Serialize.
    ///
    /// Fails with [`ErrorKind::TooLarge`] if a `StringData` field exceeds what
    /// the format can express — the `u16` character count, or MS-SHLLINK 10.0's
    /// 260-character cap on everything but the arguments — or if a fixed-width
    /// path field would not fit. Truncating instead would produce a link that
    /// silently points somewhere else.
    pub fn build(&self) -> Result<Vec<u8>> {
        let mut flags = LinkFlags::IS_UNICODE;
        if self.target_id_list.is_some() {
            flags |= LinkFlags::HAS_LINK_TARGET_ID_LIST;
        }
        if self.local_path.is_some() {
            flags |= LinkFlags::HAS_LINK_INFO;
        }
        if self.name.is_some() {
            flags |= LinkFlags::HAS_NAME;
        }
        if self.relative_path.is_some() {
            flags |= LinkFlags::HAS_RELATIVE_PATH;
        }
        if self.working_dir.is_some() {
            flags |= LinkFlags::HAS_WORKING_DIR;
        }
        if self.arguments.is_some() {
            flags |= LinkFlags::HAS_ARGUMENTS;
        }
        if self.icon_location.is_some() {
            flags |= LinkFlags::HAS_ICON_LOCATION;
        }
        if self.environment_path.is_some() {
            flags |= LinkFlags::HAS_EXP_STRING;
        }
        if self.icon_environment_path.is_some() {
            flags |= LinkFlags::HAS_EXP_ICON;
        }

        let header = ShellLinkHeader {
            link_flags: flags,
            file_attributes: self.file_attributes,
            creation_time: self.creation_time,
            access_time: self.access_time,
            write_time: self.write_time,
            file_size: self.file_size,
            icon_index: self.icon_index,
            show_command: self.show_command.unwrap_or(ShowCommand::NORMAL),
            hot_key: self.hot_key,
        };

        let mut out = Vec::new();
        out.extend_from_slice(&header.to_bytes());

        if let Some(id_list) = &self.target_id_list {
            let size = u16::try_from(id_list.len())
                .map_err(|_| Error::new(ErrorKind::TooLarge, out.len()))?;
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(id_list);
        }

        if let Some(path) = &self.local_path {
            out.extend_from_slice(&build_link_info(path, self.volume.as_ref())?);
        }

        // StringData, in the order MS-SHLLINK 2.4 fixes. Everything but the
        // arguments is capped at 260 characters by revision 10.0.
        for (value, bounded) in [
            (&self.name, true),
            (&self.relative_path, true),
            (&self.working_dir, true),
            (&self.arguments, false),
            (&self.icon_location, true),
        ] {
            let Some(s) = value else { continue };
            let count = s.encode_utf16().count();
            let count = check_writable_length(count, bounded)?;
            out.extend_from_slice(&count.to_le_bytes());
            for unit in s.encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        }

        if let Some(p) = &self.environment_path {
            out.extend_from_slice(&build_path_pair_block(SIG_ENVIRONMENT_VARIABLE, p)?);
        }
        if let Some(p) = &self.icon_environment_path {
            out.extend_from_slice(&build_path_pair_block(SIG_ICON_ENVIRONMENT, p)?);
        }
        for block in &self.extra_blocks {
            out.extend_from_slice(block);
        }
        // TerminalBlock. Required, and cheap to forget.
        out.extend_from_slice(&0u32.to_le_bytes());

        Ok(out)
    }
}

/// One of the three fixed-layout `0x314` blocks: 260 ANSI bytes then 520 UTF-16.
fn build_path_pair_block(signature: u32, path: &str) -> Result<Vec<u8>> {
    let ansi = ansi_lossy(path);
    // 260 and 520 include the terminating NUL, so the string itself gets one
    // byte and one code unit less.
    if ansi.len() >= 260 {
        return Err(Error::new(ErrorKind::TooLarge, 0));
    }
    let units: Vec<u16> = path.encode_utf16().collect();
    if units.len() >= 260 {
        return Err(Error::new(ErrorKind::TooLarge, 0));
    }

    let mut block = Vec::with_capacity(SIZE_ENVIRONMENT_VARIABLE as usize);
    block.extend_from_slice(&SIZE_ENVIRONMENT_VARIABLE.to_le_bytes());
    block.extend_from_slice(&signature.to_le_bytes());

    let mut fixed = vec![0u8; 260];
    fixed[..ansi.len()].copy_from_slice(&ansi);
    block.extend_from_slice(&fixed);

    let mut wide = vec![0u8; 520];
    for (i, unit) in units.iter().enumerate() {
        wide[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
    }
    block.extend_from_slice(&wide);

    debug_assert_eq!(block.len(), SIZE_ENVIRONMENT_VARIABLE as usize);
    Ok(block)
}

/// Build a `LinkInfo` with a `VolumeID` and a local base path.
///
/// Writes the ANSI-only 0x1C header when everything is ASCII, and the 0x24
/// header with Unicode companion fields when it is not — the same rule
/// MS-SHLLINK records Windows following, and the reason `LinkInfoHeaderSize` is
/// a `>=` test rather than an `==` one.
fn build_link_info(path: &str, volume: Option<&Volume>) -> Result<Vec<u8>> {
    let default_volume = Volume::default();
    let volume = volume.unwrap_or(&default_volume);

    let needs_unicode = !path.is_ascii();
    let header_size = if needs_unicode {
        LINK_INFO_HEADER_WITH_UNICODE
    } else {
        LINK_INFO_HEADER_MIN
    };

    let volume_id = build_volume_id(volume);

    let mut path_ansi = ansi_lossy(path);
    path_ansi.push(0);
    // CommonPathSuffix is always present and is the empty string here: the whole
    // path lives in LocalBasePath.
    let suffix_ansi = [0u8];

    let mut body = Vec::new();
    let volume_id_offset = header_size as usize;
    body.extend_from_slice(&volume_id);
    let local_base_path_offset = volume_id_offset + body.len();
    body.extend_from_slice(&path_ansi);
    let common_path_suffix_offset = volume_id_offset + body.len();
    body.extend_from_slice(&suffix_ansi);

    let (local_base_path_offset_unicode, common_path_suffix_offset_unicode) = if needs_unicode {
        let lbp = volume_id_offset + body.len();
        for unit in path.encode_utf16() {
            body.extend_from_slice(&unit.to_le_bytes());
        }
        body.extend_from_slice(&[0, 0]);
        let cps = volume_id_offset + body.len();
        body.extend_from_slice(&[0, 0]);
        (Some(lbp), Some(cps))
    } else {
        (None, None)
    };

    let total = header_size as usize + body.len();
    let size = u32::try_from(total).map_err(|_| Error::new(ErrorKind::TooLarge, 0))?;

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&header_size.to_le_bytes());
    out.extend_from_slice(&LinkInfoFlags::VOLUME_ID_AND_LOCAL_BASE_PATH.0.to_le_bytes());
    out.extend_from_slice(&(volume_id_offset as u32).to_le_bytes());
    out.extend_from_slice(&(local_base_path_offset as u32).to_le_bytes());
    // No CommonNetworkRelativeLink: the flag is clear, so the offset MUST be 0.
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(common_path_suffix_offset as u32).to_le_bytes());
    if let (Some(lbp), Some(cps)) = (
        local_base_path_offset_unicode,
        common_path_suffix_offset_unicode,
    ) {
        out.extend_from_slice(&(lbp as u32).to_le_bytes());
        out.extend_from_slice(&(cps as u32).to_le_bytes());
    }
    out.extend_from_slice(&body);

    debug_assert_eq!(out.len(), total);
    Ok(out)
}

/// A `VolumeID`, with the label written as ANSI when it can be and as Unicode
/// otherwise.
fn build_volume_id(volume: &Volume) -> Vec<u8> {
    let ascii_label = volume.label.is_ascii();
    // 0x10 for the four mandatory u32s; 0x14 once VolumeLabelOffsetUnicode is
    // there too. VolumeLabelOffset == 0x14 is the spec's sentinel for "ignore
    // me and read the Unicode offset instead", and it happens to equal the
    // header length in that case, which is not a coincidence.
    let header_len: u32 = if ascii_label { 0x10 } else { 0x14 };

    let mut label = Vec::new();
    if ascii_label {
        label.extend_from_slice(volume.label.as_bytes());
        label.push(0);
    } else {
        for unit in volume.label.encode_utf16() {
            label.extend_from_slice(&unit.to_le_bytes());
        }
        label.extend_from_slice(&[0, 0]);
    }

    let size = header_len + label.len() as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&volume.drive_type.0.to_le_bytes());
    out.extend_from_slice(&volume.serial_number.to_le_bytes());
    if ascii_label {
        out.extend_from_slice(&header_len.to_le_bytes());
    } else {
        out.extend_from_slice(&0x0000_0014u32.to_le_bytes());
        out.extend_from_slice(&header_len.to_le_bytes());
    }
    out.extend_from_slice(&label);

    debug_assert_eq!(out.len(), size as usize);
    debug_assert!(
        size > 0x10,
        "MS-SHLLINK 2.3.1: VolumeIDSize MUST be greater than 0x10"
    );
    out
}

/// ASCII bytes, with anything outside ASCII replaced by `?`.
///
/// The fixed ANSI fields in this format have no code page attached, so there is
/// no correct byte for a non-ASCII character. `?` is what Windows substitutes,
/// and it is visibly wrong rather than plausibly wrong — the Unicode companion
/// field is where the real text goes.
fn ansi_lossy(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .collect()
}

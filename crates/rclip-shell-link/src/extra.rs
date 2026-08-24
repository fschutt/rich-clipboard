//! MS-SHLLINK 2.5 — `ExtraData`.
//!
//! A chain of self-describing blocks after `StringData`, ended by a `u32` less
//! than `0x00000004`. Every block starts with its own size and a `0xA00000xx`
//! signature, and **`BlockSize` includes the size and signature fields
//! themselves** — `ConsoleFEDataBlock` is `0x0C` for four bytes of size, four of
//! signature and one `u32` of payload. So advancing the walk is `pos +=
//! BlockSize`.
//!
//! # Why this loop cannot stall
//!
//! `BlockSize` is a `u32` off the wire and it is the stride. A block that
//! declares less than eight bytes cannot even hold its own signature, so it is
//! rejected rather than skipped: skipping it would either advance by zero or
//! resume the walk in the middle of a field. Values below four are the defined
//! terminator and end the chain cleanly. Everything that reaches the dispatch
//! below has therefore advanced the cursor by at least eight bytes.
//!
//! An unknown signature is **not** an error — it becomes
//! [`ExtraDataBlock::Unknown`] with its bytes. `0xA000000A` is unassigned today
//! and Microsoft may assign it tomorrow.

use rclip_core::{Error, ErrorKind, Reader, Result};
use rclip_idlist::{Guid, ItemIdList, ShellStr};

/// `EnvironmentVariableDataBlock`.
pub const SIG_ENVIRONMENT_VARIABLE: u32 = 0xA000_0001;
/// `ConsoleDataBlock`.
pub const SIG_CONSOLE: u32 = 0xA000_0002;
/// `TrackerDataBlock`.
pub const SIG_TRACKER: u32 = 0xA000_0003;
/// `ConsoleFEDataBlock`.
pub const SIG_CONSOLE_FE: u32 = 0xA000_0004;
/// `SpecialFolderDataBlock`.
pub const SIG_SPECIAL_FOLDER: u32 = 0xA000_0005;
/// `DarwinDataBlock`.
pub const SIG_DARWIN: u32 = 0xA000_0006;
/// `IconEnvironmentDataBlock`.
pub const SIG_ICON_ENVIRONMENT: u32 = 0xA000_0007;
/// `ShimDataBlock`.
pub const SIG_SHIM: u32 = 0xA000_0008;
/// `PropertyStoreDataBlock`.
pub const SIG_PROPERTY_STORE: u32 = 0xA000_0009;
/// `KnownFolderDataBlock`. Note the jump: `0xA000000A` is unassigned.
pub const SIG_KNOWN_FOLDER: u32 = 0xA000_000B;
/// `VistaAndAboveIDListDataBlock`.
pub const SIG_VISTA_AND_ABOVE_ID_LIST: u32 = 0xA000_000C;

/// A `BlockSize` strictly below this ends the `ExtraData` chain.
pub const TERMINAL_BLOCK_MAX: u32 = 0x0000_0004;
/// Smallest block that can carry a size and a signature.
pub const MIN_BLOCK_SIZE: u32 = 8;

/// Fixed block sizes from MS-SHLLINK 2.5, verified against revision 10.0.
pub const SIZE_ENVIRONMENT_VARIABLE: u32 = 0x0000_0314;
pub const SIZE_CONSOLE: u32 = 0x0000_00CC;
pub const SIZE_TRACKER: u32 = 0x0000_0060;
pub const SIZE_CONSOLE_FE: u32 = 0x0000_000C;
pub const SIZE_SPECIAL_FOLDER: u32 = 0x0000_0010;
pub const SIZE_DARWIN: u32 = 0x0000_0314;
pub const SIZE_ICON_ENVIRONMENT: u32 = 0x0000_0314;
pub const SIZE_KNOWN_FOLDER: u32 = 0x0000_001C;
/// Minimum sizes for the three variable-length blocks.
pub const MIN_SIZE_SHIM: u32 = 0x0000_0088;
pub const MIN_SIZE_PROPERTY_STORE: u32 = 0x0000_000C;
pub const MIN_SIZE_VISTA_AND_ABOVE_ID_LIST: u32 = 0x0000_000A;

/// One `ExtraData` block.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtraDataBlock<'a> {
    /// 2.5.4 `EnvironmentVariableDataBlock` — "a path to environment variable
    /// information", i.e. the target expressed as something like
    /// `%windir%\system32\cmd.exe`.
    ///
    /// Gated by `LinkFlags::HAS_EXP_STRING`. This is how a link stays valid
    /// across machines whose drive layouts differ, and it is also the section
    /// that makes a link work with no target IDList at all.
    EnvironmentVariable(PathPair<'a>),
    /// 2.5.3 `DarwinDataBlock` — a Windows Installer application descriptor,
    /// used to install the target on activation instead of launching it.
    ///
    /// The spec says of the ANSI half: "This field SHOULD be ignored."
    Darwin(PathPair<'a>),
    /// 2.5.5 `IconEnvironmentDataBlock` — the icon's path, written with
    /// environment variables so it resolves across machines.
    IconEnvironment(PathPair<'a>),
    /// 2.5.1 `ConsoleDataBlock` — window, font and colour settings for a target
    /// that runs in a console.
    Console(ConsoleDataBlock<'a>),
    /// 2.5.2 `ConsoleFEDataBlock` — the code page to display console text in.
    ConsoleFe {
        /// A code page language code identifier; see MS-LCID.
        code_page: u32,
    },
    /// 2.5.9 `SpecialFolderDataBlock` — the target's location as a numeric
    /// special folder ID, so the IDList can be re-based when the link loads on
    /// a machine whose folders live elsewhere.
    SpecialFolder {
        special_folder_id: u32,
        /// Offset, in bytes, into the link target IDList of the first child
        /// segment below the special folder.
        offset: u32,
    },
    /// 2.5.6 `KnownFolderDataBlock` — the same idea as `SpecialFolder`, but by
    /// `KNOWNFOLDERID` GUID rather than by integer.
    KnownFolder {
        known_folder_id: Guid,
        /// Offset, in bytes, into the link target IDList.
        offset: u32,
    },
    /// 2.5.8 `ShimDataBlock` — the name of an application-compatibility shim
    /// layer to apply when the target is activated.
    Shim {
        /// A Unicode string. The spec does not say it is NUL-terminated; the
        /// length is implied by `BlockSize`.
        layer_name: ShellStr<'a>,
    },
    /// 2.5.7 `PropertyStoreDataBlock` — an MS-PROPSTORE serialized property
    /// storage. Kept opaque; see the crate README.
    PropertyStore {
        /// The serialized property storage, undecoded.
        property_store: &'a [u8],
    },
    /// 2.5.10 `TrackerDataBlock` — what the Distributed Link Tracking service
    /// needs to find a target that has moved.
    ///
    /// Also the single most useful block for forensics, because `MachineID` is
    /// the NetBIOS name of the machine the link was made on.
    Tracker(TrackerDataBlock<'a>),
    /// 2.5.11 `VistaAndAboveIDListDataBlock` — an alternate IDList for shells
    /// that understand it, used in preference to `LinkTargetIDList`.
    ///
    /// Unlike `LinkTargetIDList` this has **no** leading `u16` size; the block
    /// size bounds it.
    VistaAndAboveIdList {
        /// The raw `IDList` bytes.
        id_list: &'a [u8],
    },
    /// A signature this crate does not know.
    ///
    /// Not an error. `0xA000000A` is unassigned as of revision 10.0 and could
    /// be assigned later, and third parties have been known to append their own
    /// blocks.
    Unknown {
        signature: u32,
        /// `BlockSize` as declared.
        size: u32,
        /// Everything after the size and signature.
        body: &'a [u8],
    },
}

impl<'a> ExtraDataBlock<'a> {
    /// The block's signature, including for [`ExtraDataBlock::Unknown`].
    #[must_use]
    pub const fn signature(&self) -> u32 {
        match self {
            Self::EnvironmentVariable(_) => SIG_ENVIRONMENT_VARIABLE,
            Self::Console(_) => SIG_CONSOLE,
            Self::Tracker(_) => SIG_TRACKER,
            Self::ConsoleFe { .. } => SIG_CONSOLE_FE,
            Self::SpecialFolder { .. } => SIG_SPECIAL_FOLDER,
            Self::Darwin(_) => SIG_DARWIN,
            Self::IconEnvironment(_) => SIG_ICON_ENVIRONMENT,
            Self::Shim { .. } => SIG_SHIM,
            Self::PropertyStore { .. } => SIG_PROPERTY_STORE,
            Self::KnownFolder { .. } => SIG_KNOWN_FOLDER,
            Self::VistaAndAboveIdList { .. } => SIG_VISTA_AND_ABOVE_ID_LIST,
            Self::Unknown { signature, .. } => *signature,
        }
    }

    /// Walk the alternate IDList, for the one variant that holds one.
    #[must_use]
    pub const fn id_list(&self) -> Option<ItemIdList<'a>> {
        match self {
            Self::VistaAndAboveIdList { id_list } => Some(ItemIdList::new(id_list)),
            _ => None,
        }
    }
}

/// The ANSI + Unicode path pair shared by the environment variable, Darwin and
/// icon environment blocks: 260 bytes then 520, both NUL-terminated inside a
/// fixed-width field.
///
/// The fixed widths are why these three blocks are all `0x314` bytes regardless
/// of how short the strings are; the bytes after each terminator are undefined
/// and MUST NOT be used, which is why both halves are trimmed at the NUL here.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PathPair<'a> {
    /// The system-code-page form, trimmed at its NUL.
    pub ansi: &'a [u8],
    /// The UTF-16LE form, trimmed at its NUL. Empty when only ANSI was written.
    pub unicode: &'a [u8],
}

impl<'a> PathPair<'a> {
    /// The better of the two: Unicode when it is non-empty, ANSI otherwise.
    ///
    /// The ANSI half is written in the code page of the machine that made the
    /// link, which is not recorded anywhere in the file, so the Unicode half is
    /// the only one that reliably means the same thing elsewhere.
    #[must_use]
    pub const fn path(&self) -> ShellStr<'a> {
        if self.unicode.is_empty() {
            ShellStr::Ansi(self.ansi)
        } else {
            ShellStr::Utf16(self.unicode)
        }
    }

    fn parse(r: &mut Reader<'a>) -> Result<Self> {
        Ok(Self { ansi: trim_nul(r.take(260)?), unicode: r.utf16_fixed(260)? })
    }
}

/// MS-SHLLINK 2.5.1 — console window settings.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ConsoleDataBlock<'a> {
    /// Foreground and background colour indexes into
    /// [`ConsoleDataBlock::color_table`].
    pub fill_attributes: u16,
    /// The same, for popups.
    pub popup_fill_attributes: u16,
    /// Screen buffer size in characters. Signed on the wire.
    pub screen_buffer_size_x: i16,
    pub screen_buffer_size_y: i16,
    /// Window size in characters.
    pub window_size_x: i16,
    pub window_size_y: i16,
    /// Window origin in pixels.
    pub window_origin_x: i16,
    pub window_origin_y: i16,
    /// "The two most significant bytes contain the font height and the two
    /// least significant bytes contain the font width. For vector fonts, the
    /// width is set to zero."
    pub font_size: u32,
    /// An `FF_*` family OR'd with `TMPF_*` pitch bits.
    pub font_family: u32,
    /// 700 or above is bold.
    pub font_weight: u32,
    /// 32 UTF-16 characters, trimmed at the NUL.
    pub face_name: ShellStr<'a>,
    /// Percentage: at most 25 small, 26-50 medium, 51-100 large.
    pub cursor_size: u32,
    pub full_screen: bool,
    pub quick_edit: bool,
    pub insert_mode: bool,
    /// When false, `window_origin_x`/`_y` position the window.
    pub auto_position: bool,
    pub history_buffer_size: u32,
    pub number_of_history_buffers: u32,
    /// The spec's table for this one reads as though it were inverted; it says
    /// zero means duplicates are *not* allowed and non-zero that they are.
    pub history_no_dup: u32,
    /// The 16 RGB values the fill attributes index into.
    pub color_table: [u32; 16],
}

/// `FillAttributes` bits.
pub mod fill {
    pub const FOREGROUND_BLUE: u16 = 0x0001;
    pub const FOREGROUND_GREEN: u16 = 0x0002;
    pub const FOREGROUND_RED: u16 = 0x0004;
    pub const FOREGROUND_INTENSITY: u16 = 0x0008;
    pub const BACKGROUND_BLUE: u16 = 0x0010;
    pub const BACKGROUND_GREEN: u16 = 0x0020;
    pub const BACKGROUND_RED: u16 = 0x0040;
    pub const BACKGROUND_INTENSITY: u16 = 0x0080;
}

/// `FontFamily` values, before the `TMPF_*` pitch bits are OR'd in.
pub mod font_family {
    pub const FF_DONTCARE: u32 = 0x0000;
    pub const FF_ROMAN: u32 = 0x0010;
    pub const FF_SWISS: u32 = 0x0020;
    pub const FF_MODERN: u32 = 0x0030;
    pub const FF_SCRIPT: u32 = 0x0040;
    pub const FF_DECORATIVE: u32 = 0x0050;

    pub const TMPF_FIXED_PITCH: u32 = 0x0001;
    pub const TMPF_VECTOR: u32 = 0x0002;
    pub const TMPF_TRUETYPE: u32 = 0x0004;
    pub const TMPF_DEVICE: u32 = 0x0008;
}

impl<'a> ConsoleDataBlock<'a> {
    fn parse(r: &mut Reader<'a>) -> Result<Self> {
        let fill_attributes = r.u16_le()?;
        let popup_fill_attributes = r.u16_le()?;
        let screen_buffer_size_x = r.i16_le()?;
        let screen_buffer_size_y = r.i16_le()?;
        let window_size_x = r.i16_le()?;
        let window_size_y = r.i16_le()?;
        let window_origin_x = r.i16_le()?;
        let window_origin_y = r.i16_le()?;
        r.skip(8)?; // Unused1, Unused2 — "undefined and MUST be ignored"
        let font_size = r.u32_le()?;
        let font_family = r.u32_le()?;
        let font_weight = r.u32_le()?;
        let face_name = ShellStr::Utf16(r.utf16_fixed(32)?);
        let cursor_size = r.u32_le()?;
        let full_screen = r.u32_le()? != 0;
        let quick_edit = r.u32_le()? != 0;
        let insert_mode = r.u32_le()? != 0;
        let auto_position = r.u32_le()? != 0;
        let history_buffer_size = r.u32_le()?;
        let number_of_history_buffers = r.u32_le()?;
        let history_no_dup = r.u32_le()?;
        let mut color_table = [0u32; 16];
        for slot in &mut color_table {
            *slot = r.u32_le()?;
        }
        Ok(Self {
            fill_attributes,
            popup_fill_attributes,
            screen_buffer_size_x,
            screen_buffer_size_y,
            window_size_x,
            window_size_y,
            window_origin_x,
            window_origin_y,
            font_size,
            font_family,
            font_weight,
            face_name,
            cursor_size,
            full_screen,
            quick_edit,
            insert_mode,
            auto_position,
            history_buffer_size,
            number_of_history_buffers,
            history_no_dup,
            color_table,
        })
    }

    /// `true` if `font_weight` is in the bold range.
    #[must_use]
    pub const fn is_bold(&self) -> bool {
        self.font_weight >= 700
    }
}

/// MS-SHLLINK 2.5.10 — Distributed Link Tracking identifiers.
///
/// Historically notable outside the spec: the `Droid` GUIDs are version-1 UUIDs
/// generated from the machine's MAC address, which is how `.lnk` files have been
/// used to attribute documents to a machine. This crate surfaces them as data
/// and draws no conclusions.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TrackerDataBlock<'a> {
    /// "the size of the rest of the TrackerDataBlock structure, including this
    /// Length field." MS-SHLLINK 10.0 says this MUST be exactly `0x58`, which is
    /// the only value consistent with a fixed `BlockSize` of `0x60`.
    pub length: u32,
    /// MUST be zero.
    pub version: u32,
    /// The NetBIOS name of the machine the target was last known to be on. A
    /// NUL-terminated string in a fixed 16-byte field.
    pub machine_id: ShellStr<'a>,
    /// Two GUIDs used by the link tracking service (MS-DLTW).
    pub droid: [Guid; 2],
    /// Two more, recording the target's original identity.
    pub droid_birth: [Guid; 2],
}

impl<'a> TrackerDataBlock<'a> {
    fn parse(r: &mut Reader<'a>) -> Result<Self> {
        let length = r.u32_le()?;
        let version = r.u32_le()?;
        let machine_id = ShellStr::Ansi(trim_nul(r.take(16)?));
        let droid = [Guid::from_bytes(r.guid()?), Guid::from_bytes(r.guid()?)];
        let droid_birth = [Guid::from_bytes(r.guid()?), Guid::from_bytes(r.guid()?)];
        Ok(Self { length, version, machine_id, droid, droid_birth })
    }
}

/// Iterator over the `ExtraData` chain.
///
/// Yields `Err` at most once: any structural problem ends the walk, because
/// after a bad `BlockSize` the cursor is no longer on a block boundary and
/// everything after it would be noise.
#[derive(Debug, Clone)]
pub struct ExtraDataBlocks<'a> {
    r: Reader<'a>,
    done: bool,
}

impl<'a> ExtraDataBlocks<'a> {
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { r: Reader::new(buf), done: false }
    }

    /// Bytes consumed, including the terminal block.
    #[must_use]
    pub const fn bytes_consumed(&self) -> usize {
        self.r.pos()
    }
}

impl<'a> Iterator for ExtraDataBlocks<'a> {
    type Item = Result<ExtraDataBlock<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // A link whose ExtraData simply runs out has no terminal block. The
        // spec requires one, but refusing the whole file over four missing
        // trailing bytes would throw away everything already parsed.
        if self.r.remaining_len() == 0 {
            self.done = true;
            return None;
        }

        let at = self.r.pos();
        let size = match self.r.peek_u32_le() {
            Ok(v) => v,
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };

        if size < TERMINAL_BLOCK_MAX {
            // The terminator. Consume it so `bytes_consumed` is honest.
            let _ = self.r.skip(4);
            self.done = true;
            return None;
        }
        if size < MIN_BLOCK_SIZE {
            // 4..8: too small to hold a signature. Advancing by it would land
            // the cursor inside the next block's fields.
            self.done = true;
            return Some(Err(Error::new(ErrorKind::BadLength, at)));
        }

        let size_usize = match usize::try_from(size) {
            Ok(v) => v,
            Err(_) => {
                self.done = true;
                return Some(Err(Error::new(ErrorKind::TooLarge, at)));
            }
        };
        // The sub-reader is the containment: a block's own parser physically
        // cannot read past the block, whatever its inner fields claim.
        let mut inner = match self.r.take_reader(size_usize) {
            Ok(v) => v,
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };

        let block = parse_block(&mut inner, size, at);
        if block.is_err() {
            self.done = true;
        }
        Some(block)
    }
}

impl core::iter::FusedIterator for ExtraDataBlocks<'_> {}

fn parse_block<'a>(r: &mut Reader<'a>, size: u32, at: usize) -> Result<ExtraDataBlock<'a>> {
    r.skip(4)?; // BlockSize, already read
    let signature = r.u32_le()?;

    // Each block's declared size is checked against the spec's table before the
    // body is read. A block that is the wrong size is a block whose fields are
    // not where they are supposed to be.
    let exact = |want: u32| -> Result<()> {
        if size == want {
            Ok(())
        } else {
            Err(Error::new(ErrorKind::BadLength, at))
        }
    };
    let least = |want: u32| -> Result<()> {
        if size >= want {
            Ok(())
        } else {
            Err(Error::new(ErrorKind::BadLength, at))
        }
    };

    Ok(match signature {
        SIG_ENVIRONMENT_VARIABLE => {
            exact(SIZE_ENVIRONMENT_VARIABLE)?;
            ExtraDataBlock::EnvironmentVariable(PathPair::parse(r)?)
        }
        SIG_DARWIN => {
            exact(SIZE_DARWIN)?;
            ExtraDataBlock::Darwin(PathPair::parse(r)?)
        }
        SIG_ICON_ENVIRONMENT => {
            exact(SIZE_ICON_ENVIRONMENT)?;
            ExtraDataBlock::IconEnvironment(PathPair::parse(r)?)
        }
        SIG_CONSOLE => {
            exact(SIZE_CONSOLE)?;
            ExtraDataBlock::Console(ConsoleDataBlock::parse(r)?)
        }
        SIG_CONSOLE_FE => {
            exact(SIZE_CONSOLE_FE)?;
            ExtraDataBlock::ConsoleFe { code_page: r.u32_le()? }
        }
        SIG_SPECIAL_FOLDER => {
            exact(SIZE_SPECIAL_FOLDER)?;
            ExtraDataBlock::SpecialFolder { special_folder_id: r.u32_le()?, offset: r.u32_le()? }
        }
        SIG_KNOWN_FOLDER => {
            exact(SIZE_KNOWN_FOLDER)?;
            ExtraDataBlock::KnownFolder {
                known_folder_id: Guid::from_bytes(r.guid()?),
                offset: r.u32_le()?,
            }
        }
        SIG_TRACKER => {
            exact(SIZE_TRACKER)?;
            ExtraDataBlock::Tracker(TrackerDataBlock::parse(r)?)
        }
        SIG_SHIM => {
            least(MIN_SIZE_SHIM)?;
            ExtraDataBlock::Shim { layer_name: ShellStr::Utf16(trim_utf16_nul(r.remaining())) }
        }
        SIG_PROPERTY_STORE => {
            least(MIN_SIZE_PROPERTY_STORE)?;
            // TODO(phase-3): decode the MS-PROPSTORE serialized property
            // storage. It carries the AppUserModelID, which is what taskbar
            // pinning keys off, so it will be wanted eventually.
            ExtraDataBlock::PropertyStore { property_store: r.remaining() }
        }
        SIG_VISTA_AND_ABOVE_ID_LIST => {
            least(MIN_SIZE_VISTA_AND_ABOVE_ID_LIST)?;
            ExtraDataBlock::VistaAndAboveIdList { id_list: r.remaining() }
        }
        _ => ExtraDataBlock::Unknown { signature, size, body: r.remaining() },
    })
}

/// Trim a fixed-width field at its first NUL.
///
/// MS-SHLLINK 2: "If a string is smaller than the size of the field that
/// contains it, the bytes in the field following the terminating null character
/// are undefined and can have any value. The undefined bytes MUST NOT be used."
fn trim_nul(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

/// The same for UTF-16LE, on even boundaries.
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

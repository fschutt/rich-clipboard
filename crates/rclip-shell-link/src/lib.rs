//! Windows shell links — `.lnk` files, and the `CFSTR_SHELLLINK` clipboard
//! flavor — per [MS-SHLLINK] revision 10.0.
//!
//! [MS-SHLLINK]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/16cb4ca1-9339-4d0c-a68d-bf1d6cc0f943
//!
//! ```text
//! SHELL_LINK = SHELL_LINK_HEADER [LINKTARGET_IDLIST] [LINKINFO]
//!              [STRING_DATA] *EXTRA_DATA
//! ```
//!
//! - [`ShellLinkHeader`] — the fixed 76 bytes, §2.1.
//! - [`LinkTargetIdList`] — the shell namespace path to the target, §2.2.
//!   Delegates to [`rclip_idlist`].
//! - [`LinkInfo`] — volume, serial number and drive-letter path, §2.3.
//! - [`StringData`] — five optional counted strings, §2.4.
//! - [`ExtraDataBlock`] — eleven self-describing trailing blocks, §2.5.
//! - [`ShellLinkBuilder`] — writing one, behind the `alloc` feature.
//!
//! # This parser does not resolve anything
//!
//! **A `.lnk` parser must never act on what it reads.** No path resolution, no
//! filesystem access, no binding an IDList to a namespace extension, no
//! launching anything, ever. This crate turns bytes into data and stops.
//!
//! That is not defensive boilerplate. The shell link format has a long CVE
//! history — CVE-2010-2568, the LNK vulnerability Stuxnet used to spread from
//! USB sticks, and CVE-2017-8464, the same class of bug seven years later —
//! and in both cases the flaw was not in *parsing* a `.lnk` but in the shell
//! **acting** on what it parsed: loading the icon a parsed `.lnk` named, from
//! a path the `.lnk` chose, at display time, with no user action at all. A link
//! file is untrusted input that describes something to execute. Keeping the
//! parse and the act apart is the whole defence, and this crate is only ever the
//! parse half.
//!
//! Concretely, everything this crate hands back is data with no guarantees
//! attached: [`StringData::arguments`] is attacker-chosen text,
//! [`StringData::icon_location`] is an attacker-chosen path, the target IDList
//! can name any namespace extension installed on the machine, and a
//! [`ExtraDataBlock::Darwin`] descriptor asks the Windows Installer to install
//! something. None of it has been validated, canonicalised or checked against a
//! policy, because this crate has no idea what your policy is.
//!
//! # Parsing strategy
//!
//! Structural failures are errors; content is returned as-is. A wrong
//! `HeaderSize` or `LinkCLSID` is fatal, since anything else means guessing
//! about what the input even is. Unknown `LinkFlags` bits, an unrecognised
//! `ShowCommand`, an unknown `ExtraData` signature and an unparseable shell item
//! are all kept and surfaced, because refusing a link over a byte Microsoft has
//! not documented yet is a self-inflicted compatibility break.
//!
//! Reading borrows: every string and every block body is a view into the
//! caller's buffer. Nothing allocates unless the `alloc` feature is on, and then
//! only in the builder.
//!
//! # Example
//!
//! ```
//! # fn main() -> Result<(), rclip_core::Error> {
//! use rclip_shell_link::{ShellLink, LINK_CLSID};
//!
//! // The smallest legal shell link: a header with no optional sections.
//! let mut bytes = [0u8; 76];
//! bytes[0] = 0x4C;                                     // HeaderSize
//! bytes[4..20].copy_from_slice(&LINK_CLSID);
//! bytes[20..24].copy_from_slice(&0u32.to_le_bytes());   // LinkFlags: none
//! bytes[60..64].copy_from_slice(&1u32.to_le_bytes());   // SW_SHOWNORMAL
//!
//! let link = ShellLink::parse(&bytes)?;
//! assert!(link.target_id_list.is_none());
//! assert!(link.link_info.is_none());
//! assert!(link.string_data.is_empty());
//! assert!(link.header.hot_key.is_unset());
//! # Ok(())
//! # }
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod extra;
pub mod filetime;
pub mod header;
pub mod link_info;
pub mod string_data;
pub mod target;

#[cfg(feature = "alloc")]
pub mod write;

pub use extra::{ExtraDataBlock, ExtraDataBlocks, PathPair};
pub use filetime::FileTime;
pub use header::{
    FileAttributes, HotKey, LinkFlags, ShellLinkHeader, ShowCommand, HEADER_SIZE, LINK_CLSID,
};
pub use link_info::{
    CommonNetworkRelativeLink, CommonNetworkRelativeLinkFlags, DriveType, LinkInfo, LinkInfoFlags,
    NetworkProviderType, VolumeId,
};
pub use string_data::StringData;
pub use target::LinkTargetIdList;

#[cfg(feature = "alloc")]
pub use write::ShellLinkBuilder;

/// Re-exported so callers do not need to depend on `rclip-idlist` directly just
/// to name the type a string field comes back as.
pub use rclip_idlist::ShellStr;

use rclip_core::{Reader, Result};

/// A parsed shell link, borrowing from the buffer it was parsed from.
#[derive(Debug, Clone)]
pub struct ShellLink<'a> {
    pub header: ShellLinkHeader,
    /// §2.2. Present iff `LinkFlags::HAS_LINK_TARGET_ID_LIST`.
    pub target_id_list: Option<LinkTargetIdList<'a>>,
    /// §2.3. Present iff `LinkFlags::HAS_LINK_INFO`.
    ///
    /// Note that `LinkFlags::FORCE_NO_LINK_INFO` means "present but to be
    /// ignored when resolving" — the structure is still on disk and is still
    /// returned here. Honouring the flag is the caller's decision.
    pub link_info: Option<LinkInfo<'a>>,
    /// §2.4. Each field is `None` when its `LinkFlags` bit was clear.
    pub string_data: StringData<'a>,
    /// The bytes from the end of `StringData` to the end of the input.
    extra: &'a [u8],
}

impl<'a> ShellLink<'a> {
    /// Parse a shell link from a complete buffer.
    ///
    /// Sections are read in the order the ABNF fixes, each gated on its
    /// `LinkFlags` bit, because every one of them is variable length and there
    /// is no way to seek to a later section without having read the earlier
    /// ones. That in turn means a malformed `LinkInfo` costs you the
    /// `StringData` after it — unavoidable, and the reason the length fields in
    /// between are checked as strictly as they are.
    pub fn parse(buf: &'a [u8]) -> Result<Self> {
        let header = ShellLinkHeader::parse(buf)?;
        let mut r = Reader::new(buf);
        r.skip(HEADER_SIZE)?;

        let target_id_list = if header
            .link_flags
            .contains(LinkFlags::HAS_LINK_TARGET_ID_LIST)
        {
            Some(LinkTargetIdList::parse(&mut r)?)
        } else {
            None
        };

        let link_info = if header.link_flags.contains(LinkFlags::HAS_LINK_INFO) {
            Some(LinkInfo::parse(&mut r)?)
        } else {
            None
        };

        let string_data = StringData::parse(&mut r, header.link_flags)?;

        Ok(Self {
            header,
            target_id_list,
            link_info,
            string_data,
            extra: r.remaining(),
        })
    }

    /// Walk the `ExtraData` chain.
    #[must_use]
    pub const fn extra_data(&self) -> ExtraDataBlocks<'a> {
        ExtraDataBlocks::new(self.extra)
    }

    /// The raw `ExtraData` region, terminal block included.
    #[must_use]
    pub const fn extra_data_bytes(&self) -> &'a [u8] {
        self.extra
    }

    /// The first `ExtraData` block with this signature, if the chain is
    /// walkable that far.
    ///
    /// Convenience for the common "does this link have an environment variable
    /// block" question. Stops at the first structural error, like the iterator.
    #[must_use]
    pub fn find_extra(&self, signature: u32) -> Option<ExtraDataBlock<'a>> {
        self.extra_data()
            .map_while(Result::ok)
            .find(|b| b.signature() == signature)
    }

    /// The target as an environment-variable path, e.g.
    /// `%windir%\system32\cmd.exe`, if the link carries one.
    ///
    /// This is the closest thing a shell link has to a plain target path that
    /// is meaningful off the machine that wrote it. It is still a string chosen
    /// by whoever made the link: expanding it, resolving it or opening it is the
    /// caller's business and its risk.
    #[must_use]
    pub fn environment_path(&self) -> Option<ShellStr<'a>> {
        match self.find_extra(extra::SIG_ENVIRONMENT_VARIABLE) {
            Some(ExtraDataBlock::EnvironmentVariable(p)) => Some(p.path()),
            _ => None,
        }
    }
}

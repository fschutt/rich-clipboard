//! GUIDs in packet representation, and the handful of shell folder GUIDs that
//! carry meaning for a display path.

use core::fmt;

/// A 16-byte GUID in packet representation (MS-DTYP 2.3.4.2): `Data1` as a
/// little-endian `u32`, `Data2` and `Data3` as little-endian `u16`s, then
/// `Data4` as eight raw bytes.
///
/// Stored as raw bytes rather than as four fields so that round-tripping a
/// shell item is byte-exact even for a GUID we do not recognise.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid([u8; 16]);

impl Guid {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Parse from the first 16 bytes of `bytes`, or `None` if there are fewer.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        let head = bytes.get(..16)?;
        let mut out = [0u8; 16];
        out.copy_from_slice(head);
        Some(Self(out))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    #[must_use]
    pub const fn data1(&self) -> u32 {
        u32::from_le_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }

    #[must_use]
    pub const fn data2(&self) -> u16 {
        u16::from_le_bytes([self.0[4], self.0[5]])
    }

    #[must_use]
    pub const fn data3(&self) -> u16 {
        u16::from_le_bytes([self.0[6], self.0[7]])
    }

    /// The GUID rendered as `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` in a
    /// stack buffer.
    ///
    /// `Display` covers the same ground, but a `no_std` caller with no allocator
    /// and no formatter still needs a `&str` to compare or log.
    #[must_use]
    pub fn to_braced(&self) -> GuidStr {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        // Byte order of the printed form: the three leading integers are
        // little-endian on the wire but printed big-endian, which is exactly the
        // bug that makes hand-written GUID formatters disagree with regedit.
        const ORDER: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut buf = [b'-'; GuidStr::LEN];
        buf[0] = b'{';
        buf[GuidStr::LEN - 1] = b'}';
        let mut w = 1usize;
        for (i, &src) in ORDER.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                w += 1; // leave the '-' already in place
            }
            let b = self.0[src];
            buf[w] = HEX[usize::from(b >> 4)];
            buf[w + 1] = HEX[usize::from(b & 0x0F)];
            w += 2;
        }
        GuidStr(buf)
    }

    /// The shell's name for this folder, if it is one of the well-known ones.
    ///
    /// Not a security boundary and not exhaustive — it exists so that a root
    /// folder item can render as `This PC` instead of a GUID in a breadcrumb.
    #[must_use]
    pub fn well_known_name(&self) -> Option<&'static str> {
        let s = self.to_braced();
        WELL_KNOWN
            .iter()
            .find(|(g, _)| *g == s.as_str())
            .map(|(_, n)| *n)
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_braced().as_str())
    }
}

/// A GUID formatted as `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, on the stack.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct GuidStr([u8; Self::LEN]);

impl GuidStr {
    /// 32 hex digits + 4 dashes + 2 braces.
    pub const LEN: usize = 38;

    #[must_use]
    pub fn as_str(&self) -> &str {
        // Every byte written by `to_braced` is ASCII, so this cannot fail; the
        // fallback keeps the promise without `unsafe`.
        core::str::from_utf8(&self.0).unwrap_or("{invalid}")
    }
}

impl fmt::Debug for GuidStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for GuidStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Well-known shell folder GUIDs, as they appear in root folder shell items.
///
/// Taken from libfwsi's `libfwsi_shell_folder_identifier.c` table and from
/// `KNOWNFOLDERID` in `shlobj.h`. Deliberately short — libfwsi ships around 180
/// of these and only a handful ever turn up at the head of a clipboard PIDL.
/// Two entries deserve a second look before you "fix" them: `My Computer` and
/// `This PC` are genuinely different GUIDs, and the Fonts and Administrative
/// Tools GUIDs differ only in their final byte.
static WELL_KNOWN: &[(&str, &str)] = &[
    ("{00021400-0000-0000-C000-000000000046}", "Desktop"),
    ("{031E4825-7B94-4DC3-B131-E946B44C8DD5}", "Libraries"),
    ("{04731B67-D933-450A-90E6-4ACD2E9408FE}", "Search Folder"),
    ("{18989B1D-99B5-455B-841C-AB7C74E4DDFC}", "Videos"),
    ("{1F3427C8-5C10-4210-AA03-2EE45287D668}", "User Pinned"),
    ("{208D2C60-3AEA-1069-A2D7-08002B30309D}", "Network"),
    ("{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "My Computer"),
    (
        "{21EC2020-3AEA-1069-A2DD-08002B30309D}",
        "All Control Panel Items",
    ),
    (
        "{2227A280-3AEA-1069-A2DE-08002B30309D}",
        "Printers and Faxes",
    ),
    ("{26EE0668-A00A-44D7-9371-BEB064C98683}", "Control Panel"),
    ("{33E28130-4E1E-4676-835A-98395C3BC3BB}", "Pictures"),
    ("{374DE290-123F-4565-9164-39C4925E467B}", "Downloads"),
    ("{450D8FBA-AD25-11D0-98A8-0800361B1103}", "My Documents"),
    ("{4BD8D571-6D19-48D3-BE97-422220080E43}", "Music"),
    ("{59031A47-3F72-44A7-89C5-5595FE6B30EE}", "Users Files"),
    ("{5E591A74-DF96-48D3-8D67-1733BCEE28BA}", "Delegate folder"),
    ("{5E5F29CE-E0A8-49D3-AF32-7A7BDC173478}", "This PC"),
    ("{645FF040-5081-101B-9F08-00AA002F954E}", "Recycle Bin"),
    ("{679F85CB-0220-4080-B29B-5540CC05AAB6}", "Quick Access"),
    ("{871C5380-42A0-1069-A2EA-08002B30309D}", "Internet Folder"),
    ("{9343812E-1C37-4A49-A12E-4B2D810D956B}", "Search Home"),
    (
        "{B155BDF8-02F0-451E-9A26-AE317CFD7779}",
        "Computer delegate folder",
    ),
    ("{B4BFCC3A-DB2C-424C-B029-7FE99A87C641}", "Desktop"),
    ("{BD84B380-8CA2-1069-AB1D-08000948F534}", "Fonts"),
    ("{D20EA4E1-3957-11D2-A40B-0C5020524152}", "Fonts"),
    (
        "{D20EA4E1-3957-11D2-A40B-0C5020524153}",
        "Administrative Tools",
    ),
    (
        "{DFFACDC5-679F-4156-8947-C5C76BC0B67F}",
        "Users Files delegate folder",
    ),
    (
        "{E88DCCE0-B7B3-11D1-A9F0-00AA0060FA31}",
        "Compressed Folder",
    ),
    ("{ED228FDF-9EA8-4870-83B1-96B02CFE0D52}", "Games Explorer"),
    (
        "{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}",
        "Computers and Devices",
    ),
    ("{F5FB2C77-0E2F-4A16-A381-3E560C68BC83}", "Removable Drives"),
    (
        "{FBF23B42-E3F0-101B-8488-00AA003E56F8}",
        "Internet Explorer",
    ),
    ("{FDD39AD0-238F-46AF-ADB4-6C85480369C7}", "Documents"),
];

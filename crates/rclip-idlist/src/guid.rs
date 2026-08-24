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

    /// The shell's name for this control panel item, if it is one of the ones
    /// libfwsi has catalogued.
    ///
    /// Separate from [`Guid::well_known_name`] because the two namespaces
    /// overlap and disagree: `{BD84B380-8CA2-1069-AB1D-08000948F534}` is
    /// `Fonts` as a shell folder and `Font Folder` as a control panel item, and
    /// `{2227A280-3AEA-1069-A2DE-08002B30309D}` is `Printers and Faxes` in one
    /// table and `Printers` in the other. Looking a control panel GUID up in
    /// the shell folder table is therefore not merely incomplete, it is wrong.
    #[must_use]
    pub fn control_panel_name(&self) -> Option<&'static str> {
        let s = self.to_braced();
        CONTROL_PANEL
            .iter()
            .find(|(g, _)| *g == s.as_str())
            .map(|(_, n)| *n)
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

/// Control panel item identifiers, as they appear in class `0x71` shell items.
///
/// Taken from libfwsi's `libfwsi_control_panel_item_identifier.c`. Unlike
/// [`WELL_KNOWN`], this table is not an optional nicety: a control panel item's
/// body is a GUID and nothing else, so without the table the item has no
/// display name at all. The all-`FF` "unknown" sentinel libfwsi carries is left
/// out — it is a lookup-failure marker, not an identifier.
static CONTROL_PANEL: &[(&str, &str)] = &[
    (
        "{00F2886F-CD64-4FC9-8EC5-30EF6CDBE8C3}",
        "Scanner and Camera Control Panel",
    ),
    ("{0142E4D0-FB7A-11DC-BA4A-000FFE7AB428}", "Biometrics"),
    ("{025A5937-A6BE-4686-A844-36FE4BEC8B6D}", "Power Options"),
    (
        "{05D7B0F4-2121-4EFF-BF6B-ED3F69B894D9}",
        "Taskbar Notification Icons Control Panel",
    ),
    (
        "{09F581A3-1F64-4E5B-8DB3-88F5593080CC}",
        "x86 Control Panel",
    ),
    (
        "{0DF44EAA-FF21-4412-828E-260A8728E7F1}",
        "Taskbar and Start Menu",
    ),
    (
        "{1206F5F1-0569-412C-8FEC-3204630DFB70}",
        "Credential Manager",
    ),
    (
        "{15EAE92E-F17A-4431-9F28-805E482DAFD4}",
        "Install New Programs",
    ),
    (
        "{17CD9488-1228-4B2F-88CE-4298E93E0966}",
        "Set User Defaults",
    ),
    ("{2227A280-3AEA-1069-A2DE-08002B30309D}", "Printers"),
    (
        "{241D7C96-F8BF-4F85-B01F-E2B043341A4B}",
        "Workspaces Center",
    ),
    (
        "{335A31DD-F04B-4D76-A925-D6B47CF360DF}",
        "Backup and Restore Center",
    ),
    ("{36EEF7DB-88AD-4E81-AD49-0E313F0C35F8}", "Windows Update"),
    (
        "{37EFD44D-EF8D-41B1-940D-96973A50E9E0}",
        "Windows Sidebar Properties",
    ),
    (
        "{38A98528-6CBF-4CA9-8DC0-B1E1D10F7B1B}",
        "View Available Networks",
    ),
    (
        "{3E7EFB4C-FAF1-453D-89EB-56026875EF90}",
        "Get Programs Online",
    ),
    ("{4026492F-2F69-46B8-B9BF-5654FC07E423}", "Windows Firewall"),
    ("{40419485-C444-4567-851A-2DD7BFA1684D}", "Phone and Modem"),
    (
        "{58E3C745-D971-4081-9034-86E34B30836A}",
        "Speech Recognition",
    ),
    ("{5EA4F148-308C-46D7-98A9-49041B1DD468}", "Mobility Center"),
    ("{60632754-C523-4B62-B45C-4172DA012619}", "User Accounts"),
    (
        "{62D8ED13-C9D0-4CE8-A914-47DD628FB1B0}",
        "Region and Language",
    ),
    (
        "{640167B4-59B0-47A6-B335-A6B3C0695AEA}",
        "Portable Media Devices",
    ),
    (
        "{67CA7650-96E6-4FDD-BB43-A8E774F73A57}",
        "HomeGroup Control Panel",
    ),
    ("{6C8EEC18-8D75-41B2-A177-8831D59D2D50}", "Mouse"),
    ("{6DFD7C5C-2451-11D3-A299-00C04F8EF6AF}", "Folder Options"),
    (
        "{7007ACC7-3202-11D1-AAD2-00805FC1270E}",
        "Network Connections",
    ),
    ("{725BE8F7-668E-4C7B-8F90-46BDB0936430}", "Keyboard"),
    ("{74246BFC-4C96-11D0-ABEF-0020AF6B0B7A}", "Device Manager"),
    ("{78CB147A-98EA-4AA6-B0DF-C8681F69341C}", "CardSpace"),
    (
        "{78F3955E-3B90-4184-BD14-5397C15F1EFC}",
        "Performance Information and Tools",
    ),
    ("{7A979262-40CE-46FF-AEEE-7884AC3B6136}", "Add New Hardware"),
    (
        "{7B81BE6A-CE2B-4676-A29E-EB907A5126C5}",
        "Programs and Features",
    ),
    (
        "{80F3F1D5-FECA-45F3-BC32-752C152E456E}",
        "Tablet PC Settings",
    ),
    ("{87D66A43-7B11-4A28-9811-C86EE395ACF7}", "Indexing Options"),
    (
        "{8E0C279D-0BD1-43C3-9EBD-31C3DC5B8A77}",
        "Portable Workspace Creator",
    ),
    (
        "{8E908FC9-BECC-40F6-915B-F4CA0E70D03D}",
        "Network and Sharing Center",
    ),
    (
        "{96AE8D84-A250-4520-95A5-A47A7E3C548B}",
        "Parental Controls",
    ),
    (
        "{98F2AB62-0E29-4E4C-8EE7-B542E66740B1}",
        "Company Settings Sync",
    ),
    (
        "{992CFFA0-F557-101A-88EC-00DD010CCC48}",
        "Dial-Up Networking",
    ),
    ("{9C60DE1E-E5FC-40F4-A487-460851A8D915}", "AutoPlay"),
    (
        "{9C73F5E5-7AE7-4E32-A8E8-8D23B85255BF}",
        "Sync Center Folder",
    ),
    ("{9FE63AFD-59CF-4419-9775-ABCC3849F861}", "Recovery"),
    ("{A0275511-0E86-4ECA-97C2-ECD8F1221D08}", "Infrared"),
    ("{A304259D-52B8-4526-8B1A-A1D6CECC8243}", "iSCSI Initiator"),
    ("{A3DD4F92-658A-410F-84FD-6FBBBEF2FFFE}", "Internet Options"),
    ("{A8A91A66-3A7D-4424-8D24-04E180695C7A}", "Device Center"),
    ("{B2C761C6-29BC-4F19-9251-E6195265BAF1}", "Color Management"),
    (
        "{B98A2BEA-7D42-4558-8BD1-832F41BAC6FD}",
        "Backup And Restore",
    ),
    ("{BB06C0E4-D293-4F75-8A90-CB05B6477EEE}", "System"),
    (
        "{BB64F8A7-BEE7-4E1A-AB8D-7D8273F7FDB6}",
        "Action Center CPL",
    ),
    ("{BD84B380-8CA2-1069-AB1D-08000948F534}", "Font Folder"),
    (
        "{BE122A0E-4503-11DA-8BDE-F66BAD1E3F3A}",
        "Windows Anytime Upgrade",
    ),
    (
        "{BF782CC9-5A52-4A17-806C-2A894FFEEAC5}",
        "Language Settings",
    ),
    ("{C555438B-3C23-4769-A71F-B6D3D9B6053A}", "Display"),
    ("{C58C4893-3BE0-4B45-ABB5-A63E4B8C8651}", "Troubleshooting"),
    ("{CB1B7F8C-C50A-4176-B604-9E24DEE8D4D1}", "Welcome Center"),
    ("{D17D1D6D-CC3F-4815-8FE3-607E7D5D10B3}", "Text to Speech"),
    ("{D20EA4E1-3957-11D2-A40B-0C5020524152}", "Fonts"),
    (
        "{D20EA4E1-3957-11D2-A40B-0C5020524153}",
        "Administrative Tools",
    ),
    ("{D555645E-D4F8-4C29-A827-D93C859C4F2A}", "Ease of Access"),
    ("{D6277990-4C6A-11CF-8D87-00AA0060F5BF}", "Scheduled Tasks"),
    ("{D8559EB9-20C0-410E-BEDA-7ED416AECC2A}", "Windows Defender"),
    ("{D9EF8727-CAC2-4E60-809E-86F80A666C91}", "Secure Startup"),
    (
        "{E211B736-43FD-11D1-9EFB-0000F8757FCD}",
        "Scanners & Cameras",
    ),
    ("{E2E7934B-DCE5-43C4-9576-7FE4F75E7480}", "Date and Time"),
    ("{E7DE9B1A-7533-4556-9484-B26FB486475E}", "Network Map"),
    ("{E95A4861-D57A-4BE1-AD0F-35267E261739}", "Windows SideShow"),
    ("{E9950154-C418-419E-A90A-20C5287AE24B}", "Sensors"),
    ("{ECDB0924-4208-451E-8EE0-373C0956DE16}", "ECS"),
    ("{ED834ED6-4B5A-4BFE-8F11-A626DCB6A921}", "Personalization"),
    ("{F2DDFC82-8F12-4CDD-B7DC-D4FE1425AA4D}", "Sound"),
    ("{F6B6E965-E9B2-444B-9286-10C9152EDBC5}", "History Vault"),
    ("{F82DF8F7-8B9F-442E-A48C-818EA735FF9B}", "Pen and Touch"),
    ("{F942C606-0914-47AB-BE56-1321B8035096}", "Storage Spaces"),
    (
        "{FCFEECAE-EE1B-4849-AE50-685DCF7717EC}",
        "Problem Reports and Solutions",
    ),
];

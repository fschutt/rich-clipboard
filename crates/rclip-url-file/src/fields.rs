//! Typed readings of the numeric `[InternetShortcut]` keys.
//!
//! Every one of these is a decimal or hex string in the file, so every one is
//! fallible and every one returns an offset on failure. None of them is
//! required, so the accessors on [`crate::UrlFile`] return
//! `Option<Result<..>>`: absent and malformed are different answers.

use rclip_core::{Error, ErrorKind, Result};

/// `HotKey=` — a Win32 hot-key word.
///
/// Low byte is a virtual-key code, high byte is a `HOTKEYF_*` modifier mask.
/// This is the same encoding as `ShellLinkHeader.HotKey` in MS-SHLLINK, which
/// is what lets a `.url` and a `.lnk` agree on a shortcut key. Verified against
/// the table in the cyanwerks write-up: `833` is `0x0341`, i.e. `'A'` with
/// `SHIFT|CONTROL`, which that table lists as `Ctrl + Shift + A`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct HotKey {
    /// Virtual-key code (`VK_*`). `0x41`..=`0x5A` are `A`..=`Z`,
    /// `0x70`..=`0x7B` are `F1`..=`F12`.
    pub key: u8,
    /// `HOTKEYF_*` modifier bits; see [`HotKey::SHIFT`] and friends.
    pub modifiers: u8,
}

impl HotKey {
    /// `HOTKEYF_SHIFT`.
    pub const SHIFT: u8 = 0x01;
    /// `HOTKEYF_CONTROL`.
    pub const CONTROL: u8 = 0x02;
    /// `HOTKEYF_ALT`.
    pub const ALT: u8 = 0x04;
    /// `HOTKEYF_EXT` — the key is an extended-keyboard key.
    pub const EXTENDED: u8 = 0x08;

    /// Split a raw hot-key word into key and modifiers.
    #[must_use]
    pub const fn from_word(word: u16) -> Self {
        Self { key: (word & 0x00FF) as u8, modifiers: ((word >> 8) & 0x00FF) as u8 }
    }

    /// Recombine into the word as stored.
    #[must_use]
    pub const fn to_word(self) -> u16 {
        (self.modifiers as u16) << 8 | self.key as u16
    }

    /// `true` if the given `HOTKEYF_*` bit is set.
    #[must_use]
    pub const fn has(self, flag: u8) -> bool {
        self.modifiers & flag != 0
    }

    /// `true` if no key is assigned. Files commonly carry `HotKey=0`.
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.key == 0 && self.modifiers == 0
    }
}

/// `ShowCommand=` — the `SW_*` window state to open the target with.
///
/// Kept as the raw number rather than an enum: the value is written by whatever
/// produced the file and the `SW_*` space is larger than the three values the
/// unofficial documentation lists, so mapping unknown values to an error would
/// throw away a perfectly readable file.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ShowCommand(pub u32);

impl ShowCommand {
    /// `SW_SHOWNORMAL`. Also the behaviour when the key is absent.
    pub const NORMAL: Self = Self(1);
    /// `SW_SHOWMAXIMIZED`.
    pub const MAXIMIZED: Self = Self(3);
    /// `SW_SHOWMINNOACTIVE` — what `.url` and `.lnk` both write for "minimized".
    pub const MIN_NO_ACTIVE: Self = Self(7);
}

/// `Modified=` — a hex-encoded `FILETIME` plus a trailing byte.
///
/// The bytes are stored in little-endian order, so the hex string reads
/// backwards compared to the usual `FILETIME` presentation. The unofficial
/// documentation calls the ninth byte a checksum; nothing here depends on it,
/// so it is handed back raw rather than validated.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Modified<'a> {
    /// 100-nanosecond intervals since 1601-01-01 UTC.
    pub filetime: u64,
    /// Whatever hex digits followed the eight `FILETIME` bytes, verbatim.
    pub trailing: &'a str,
}

/// Parse a `Modified=` value at `offset`.
pub(crate) fn modified(v: &str, offset: usize) -> Result<Modified<'_>> {
    // Eight little-endian bytes is sixteen hex digits; anything shorter is not
    // a FILETIME no matter how it is interpreted.
    let head = v.get(..16).ok_or(Error::new(ErrorKind::BadLength, offset))?;
    let mut bytes = [0u8; 8];
    for (i, slot) in bytes.iter_mut().enumerate() {
        let pair = head.get(i * 2..i * 2 + 2).ok_or(Error::new(ErrorKind::BadLength, offset))?;
        *slot = hex_byte(pair).ok_or(Error::new(ErrorKind::Malformed, offset))?;
    }
    let trailing = v.get(16..).unwrap_or("");
    if !trailing.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::new(ErrorKind::Malformed, offset));
    }
    Ok(Modified { filetime: u64::from_le_bytes(bytes), trailing })
}

fn hex_byte(pair: &str) -> Option<u8> {
    let b = pair.as_bytes();
    let hi = (*b.first()?).to_digit_16()?;
    let lo = (*b.get(1)?).to_digit_16()?;
    Some(hi << 4 | lo)
}

trait Hex {
    fn to_digit_16(self) -> Option<u8>;
}

impl Hex for u8 {
    fn to_digit_16(self) -> Option<u8> {
        match self {
            b'0'..=b'9' => Some(self - b'0'),
            b'a'..=b'f' => Some(self - b'a' + 10),
            b'A'..=b'F' => Some(self - b'A' + 10),
            _ => None,
        }
    }
}

/// Parse a decimal (or `0x`-prefixed hex) unsigned value.
///
/// `HotKey=` is decimal in every file anyone has documented, but `IconIndex=`
/// turns up as `0x0` often enough that rejecting it would be pedantry.
pub(crate) fn uint(v: &str, offset: usize) -> Result<u32> {
    let (digits, radix) = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(rest) => (rest, 16),
        None => (v, 10),
    };
    if digits.is_empty() {
        return Err(Error::new(ErrorKind::Malformed, offset));
    }
    u32::from_str_radix(digits, radix).map_err(|_| Error::new(ErrorKind::Malformed, offset))
}

/// Parse a signed decimal value. `IconIndex` is documented as an index but
/// Windows also uses negative values as resource IDs, so it is signed.
pub(crate) fn int(v: &str, offset: usize) -> Result<i32> {
    let (neg, digits) = match v.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, v),
    };
    // Widen before negating: `-2147483648` is a legal `i32` but `2147483648`
    // is not, so negating after the narrowing conversion would reject it.
    let n = i64::from(uint(digits, offset)?);
    let n = if neg { -n } else { n };
    i32::try_from(n).map_err(|_| Error::new(ErrorKind::TooLarge, offset))
}

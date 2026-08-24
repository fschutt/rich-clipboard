//! Code-page decoding for `\'hh` bytes and unescaped high bytes.
//!
//! Phase 0 implements Windows-1252 and Latin-1 and nothing else. That is not
//! laziness about coverage — it is where the clipboard payloads actually are.
//! Windows writes `CF_RTF` with `\ansi\ansicpg1252` and Cocoa writes
//! `\ansi\ansicpg1252` too, and both escape anything outside the code page as
//! `\uN`, so the `\'hh` path almost never carries non-Latin text in practice.
//!
//! Everything else decodes high bytes to U+FFFD rather than guessing. A wrong
//! guess produces mojibake that looks like real text and survives into the
//! user's document; a replacement character is at least visibly a gap.
//!
//! `// TODO(phase-1):` Mac Roman (`\mac`, `\ansicpg10000`), CP437/CP850
//! (`\pc`/`\pca`), and the CJK code pages Word emits for `\fcharset` fonts.

/// The code page in effect for `\'hh` bytes.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Codepage {
    /// `\ansicpg1252`, and the assumed default for `\ansi`.
    Windows1252,
    /// ISO-8859-1: `\ansicpg28591`, `\ansicpg819`.
    Latin1,
    /// Recognised as a number but not implemented; high bytes become U+FFFD.
    Unsupported(u16),
}

impl Default for Codepage {
    /// `\ansi` with no `\ansicpg` means "the writer's system code page", which
    /// is unknowable from the bytes. 1252 is the only defensible guess for
    /// clipboard RTF: it is what every Windows and macOS writer in scope emits.
    fn default() -> Self {
        Self::Windows1252
    }
}

impl Codepage {
    /// Map an `\ansicpgN` parameter.
    #[must_use]
    pub const fn from_ansicpg(n: u16) -> Self {
        match n {
            1252 => Self::Windows1252,
            819 | 28591 => Self::Latin1,
            other => Self::Unsupported(other),
        }
    }

    /// `true` if this code page decodes every byte losslessly.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        !matches!(self, Self::Unsupported(_))
    }

    /// Decode one byte. Returns `None` for a byte this code page leaves
    /// undefined, so the caller can decide between U+FFFD and an error.
    #[must_use]
    pub const fn decode(self, b: u8) -> Option<char> {
        if b < 0x80 {
            // Every code page in scope is ASCII-transparent below 0x80.
            return char::from_u32(b as u32);
        }
        match self {
            // Only 0x80..=0x9F needs a table. 0xA0..=0xFF is identical to
            // Latin-1, and the range pattern is what keeps the subscript below
            // provably inside the 32-entry table.
            Self::Windows1252 => match b {
                0x80..=0x9F => {
                    let cp = CP1252_HIGH[(b - 0x80) as usize];
                    if cp == 0 {
                        None
                    } else {
                        char::from_u32(cp as u32)
                    }
                }
                _ => char::from_u32(b as u32),
            },
            Self::Latin1 => char::from_u32(b as u32),
            Self::Unsupported(_) => None,
        }
    }

    /// Decode one byte, substituting U+FFFD for anything undefined.
    #[must_use]
    pub const fn decode_lossy(self, b: u8) -> char {
        match self.decode(b) {
            Some(c) => c,
            None => char::REPLACEMENT_CHARACTER,
        }
    }
}

/// Windows-1252 0x80..=0x9F. `0` marks the five undefined positions
/// (0x81, 0x8D, 0x8F, 0x90, 0x9D); 0xA0..=0xFF is identical to Latin-1 and
/// needs no table.
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0x0000, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, // 80-87
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x0000, 0x017D, 0x0000, // 88-8F
    0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, // 90-97
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x0000, 0x017E, 0x0178, // 98-9F
];

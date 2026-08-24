//! Code-page decoding for `\'hh` bytes and unescaped high bytes.
//!
//! Windows-1252 and Latin-1 are built in and everything else is behind a
//! feature. That is not laziness about coverage — it is where the clipboard
//! payloads actually are.
//! Windows writes `CF_RTF` with `\ansi\ansicpg1252` and Cocoa writes
//! `\ansi\ansicpg1252` too, and both escape anything outside the code page as
//! `\uN`, so the `\'hh` path almost never carries non-Latin text in practice.
//!
//! So by default everything else decodes high bytes to U+FFFD rather than
//! guessing. A wrong guess produces mojibake that looks like real text and
//! survives into the user's document; a replacement character is at least
//! visibly a gap.
//!
//! The optional, default-off `codepage` feature lifts that restriction: with it
//! on, [`Codepage::Unsupported`] resolves through `rclip-codepage`, so `\mac`
//! (10000), `\pc` (437), `\pca` (850) and every `\ansicpgN` that names a
//! Windows-125x page decode too. What does *not* change is the refusal to
//! guess: a code page nothing implements still yields `None`, and `\ansi` with
//! no `\ansicpg` is still read as 1252 because that is what every writer in
//! scope emits.
//!
//! `// TODO(phase-2):` the CJK code pages Word emits for `\fcharset` fonts.
//! Those are multi-byte and stateful, so they need a different shape of decoder
//! than a 128-entry table.

/// The code page in effect for `\'hh` bytes.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Codepage {
    /// `\ansicpg1252`, and the assumed default for `\ansi`.
    Windows1252,
    /// ISO-8859-1: `\ansicpg28591`, `\ansicpg819`.
    Latin1,
    /// Recognised as a number but not one of the two built-in tables.
    ///
    /// The name is from Phase 0, when this always meant "high bytes become
    /// U+FFFD". With the `codepage` feature it no longer does: the number is
    /// resolved through `rclip-codepage`, so `Unsupported(10000)`,
    /// `Unsupported(437)` and `Unsupported(850)` all decode. It is kept as one
    /// variant carrying the raw `\ansicpg` number rather than split into
    /// thirteen so that adding an encoding cannot break a caller's `match`, and
    /// so that a number nothing implements is still reported as the number it
    /// was. Ask [`Codepage::is_supported`], not the variant name.
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

    /// `true` if this code page has a table to decode with.
    ///
    /// Not the same as "decodes every byte": several Windows-125x pages leave
    /// some byte values undefined, and [`Codepage::decode`] returns `None` for
    /// those in a code page that is otherwise fully supported.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        match self {
            Self::Windows1252 | Self::Latin1 => true,
            #[cfg(feature = "codepage")]
            Self::Unsupported(n) => {
                rclip_codepage::Encoding::from_windows_codepage(n as u32).is_some()
            }
            #[cfg(not(feature = "codepage"))]
            Self::Unsupported(_) => false,
        }
    }

    /// The `rclip-codepage` encoding this maps to, if any.
    ///
    /// The escape hatch for a caller that wants to decode a whole field at once
    /// rather than a byte at a time.
    #[cfg(feature = "codepage")]
    #[must_use]
    pub const fn encoding(self) -> Option<rclip_codepage::Encoding> {
        match self {
            Self::Windows1252 => Some(rclip_codepage::Encoding::Windows1252),
            Self::Latin1 => Some(rclip_codepage::Encoding::Iso8859_1),
            Self::Unsupported(n) => rclip_codepage::Encoding::from_windows_codepage(n as u32),
        }
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
            // With the `codepage` feature this is a table lookup; without it,
            // still a refusal to guess. Either way it is never a silent U+FFFD:
            // `decode_lossy` is where that decision is made, by the caller.
            #[cfg(feature = "codepage")]
            Self::Unsupported(n) => match rclip_codepage::Encoding::from_windows_codepage(n as u32)
            {
                Some(enc) => enc.decode_byte(b),
                None => None,
            },
            #[cfg(not(feature = "codepage"))]
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

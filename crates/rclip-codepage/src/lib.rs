//! Legacy single-byte code pages, for the clipboard formats that still carry them.
//!
//! Five codecs in this workspace independently hit the same wall: a payload
//! that says "these bytes are text" without saying in which encoding, or saying
//! it only as a number. `CF_HDROP` with `fWide == 0`, `.lnk` `StringData` with
//! `IsUnicode` clear, ANSI shell strings inside an `ITEMIDLIST`,
//! `[InternetShortcut.A]` in a `.url`, and RTF's `\ansicpgN` / `\mac` / `\pc` /
//! `\pca` all hand you bytes in a code page the *writer's* machine was
//! configured for. This crate is the table those five need, and nothing else.
//!
//! # What it is not
//!
//! Not a charset detector. Nothing here guesses which encoding a byte slice is
//! in; the caller must know, from `CF_LOCALE`, from `\ansicpgN`, from the
//! transport, or from the user. Guessing produces mojibake that looks like text
//! and survives into the user's document, which is worse than a visible gap.
//!
//! Not a multi-byte decoder either. Shift-JIS, GBK, Big5 and the other DBCS
//! pages Windows numbers alongside these are stateful and need a different
//! shape of parser. [`Encoding::from_windows_codepage`] returns `None` for them
//! rather than pretending.
//!
//! # The traps this crate exists to get right
//!
//! - **Windows-1252 is not ISO-8859-1.** `0x80..=0x9F` are C1 control
//!   characters in Latin-1 and printable punctuation in Windows-1252 — the
//!   euro sign, the curly quotes, the em dash. Decoding one as the other is the
//!   single most common source of mojibake in clipboard text, and both are in
//!   scope here precisely so a caller has to name which one it means.
//! - **Undefined bytes are undefined, not U+FFFD.** Seven of the nine
//!   Windows-125x pages leave some byte values unassigned — twenty-three of
//!   them in Windows-1255. [`Encoding::decode_byte`] returns `None` there and
//!   [`Decoder`] yields [`ErrorKind::Malformed`]; substituting a replacement
//!   character is available, but only by asking for it by name.
//! - **Combining marks are normal output.** Windows-1255 (Hebrew points),
//!   Windows-1256 (Arabic marks) and Windows-1258 (Vietnamese tone marks) all
//!   map ordinary bytes to combining characters. Every mapping in these tables
//!   is still exactly one byte to exactly one `char`, so the iterators stay
//!   one-to-one — but the resulting text has more `char`s than it has grapheme
//!   clusters, and a caller that truncates by `char` count will split a letter
//!   from its mark.
//!
//! # Shape
//!
//! Mirrors [`rclip_core::utf16::Utf16Le`]: an iterator of `Result<char, Error>`
//! that reports what it could not decode rather than papering over it, with the
//! owning conveniences behind the `alloc` feature.
//!
//! ```
//! use rclip_codepage::Encoding;
//!
//! // \ansicpg1252, the "smart quotes" range.
//! let enc = Encoding::from_windows_codepage(1252).unwrap();
//! assert_eq!(enc.decode_byte(0x93), Some('\u{201C}')); // left double quote
//! assert_eq!(enc.decode_byte(0x81), None);             // undefined, not U+FFFD
//!
//! // The same byte under Latin-1 is a C1 control, not a quote.
//! let latin1 = Encoding::from_windows_codepage(28591).unwrap();
//! assert_eq!(latin1.decode_byte(0x93), Some('\u{0093}'));
//! ```
//!
//! # Where the tables come from
//!
//! Generated from the Unicode Consortium's vendor mapping files by
//! `generate/generate.py`, which is checked in and pins each source file by
//! SHA-256. See [`tables`] for the per-table provenance.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod decode;
pub mod tables;

pub use decode::{Decoder, LossyDecoder};
pub use rclip_core::{Error, ErrorKind, Result};

/// A single-byte legacy encoding.
///
/// Every variant is ASCII-transparent below `0x80` — verified by the generator
/// against the upstream mapping file, not assumed — so only the high half
/// differs between them.
///
/// `#[non_exhaustive]`: the CJK and remaining ISO-8859 pages are a plausible
/// later addition, and a downstream `match` should not break when one lands.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Encoding {
    /// ISO-8859-1 / Latin-1, code pages 28591 and 819.
    ///
    /// The identity map onto U+0000..=U+00FF, C1 controls included. Callers ask
    /// for it by name often enough to be worth a variant even though it needs
    /// no table. It is *not* what a Windows producer means by "ANSI" — see
    /// [`Encoding::Windows1252`].
    Iso8859_1,
    /// Windows-1250, Central European (Latin 2). Polish, Czech, Hungarian.
    Windows1250,
    /// Windows-1251, Cyrillic. Russian, Ukrainian, Bulgarian, Serbian.
    Windows1251,
    /// Windows-1252, Western European.
    ///
    /// The default meaning of RTF `\ansi`, and the ANSI code page of every
    /// Western Windows install. Differs from ISO-8859-1 exactly on
    /// `0x80..=0x9F`, which is where the euro sign, the curly quotes and the
    /// dashes live.
    Windows1252,
    /// Windows-1253, Greek.
    Windows1253,
    /// Windows-1254, Turkish (Latin 5).
    Windows1254,
    /// Windows-1255, Hebrew.
    ///
    /// `0xC0..=0xC9` and `0xCB..=0xD1` are Hebrew points — combining marks. One
    /// byte still yields one `char`, but the text is not one `char` per
    /// grapheme.
    Windows1255,
    /// Windows-1256, Arabic.
    ///
    /// The only Windows-125x page in this crate with no undefined byte at all:
    /// all 256 values map.
    Windows1256,
    /// Windows-1257, Baltic. Estonian, Latvian, Lithuanian.
    Windows1257,
    /// Windows-1258, Vietnamese.
    ///
    /// `0xCC`, `0xEC`, `0xDE`, `0xF2` and `0xFE` are combining tone marks; the
    /// same one-byte-one-`char` caveat as [`Encoding::Windows1255`] applies.
    Windows1258,
    /// Mac OS Roman, code page 10000.
    ///
    /// RTF `\mac`, and the encoding of strings in pre-OS-X resource-fork data.
    /// `0xF0` is Apple's logo, which has no Unicode character and maps into the
    /// private use area at U+F8FF.
    MacRoman,
    /// IBM/OEM code page 437, the original US PC-DOS set. RTF `\pc`.
    ///
    /// Every byte is defined; `0x80..=0xFF` is accented Latin, box drawing and
    /// mathematics.
    Cp437,
    /// IBM/OEM code page 850, "DOS Latin 1". RTF `\pca`.
    ///
    /// Every byte is defined. Trades most of CP437's box-drawing characters for
    /// the accented letters Western Europe needed.
    Cp850,
}

impl Encoding {
    /// Every encoding this crate implements, in declaration order.
    ///
    /// Exists so a test can sweep all of them; iteration order is not API.
    pub const ALL: &'static [Self] = &[
        Self::Iso8859_1,
        Self::Windows1250,
        Self::Windows1251,
        Self::Windows1252,
        Self::Windows1253,
        Self::Windows1254,
        Self::Windows1255,
        Self::Windows1256,
        Self::Windows1257,
        Self::Windows1258,
        Self::MacRoman,
        Self::Cp437,
        Self::Cp850,
    ];

    /// The IANA / WHATWG preferred name, for `charset=` parameters and logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Iso8859_1 => "ISO-8859-1",
            Self::Windows1250 => "windows-1250",
            Self::Windows1251 => "windows-1251",
            Self::Windows1252 => "windows-1252",
            Self::Windows1253 => "windows-1253",
            Self::Windows1254 => "windows-1254",
            Self::Windows1255 => "windows-1255",
            Self::Windows1256 => "windows-1256",
            Self::Windows1257 => "windows-1257",
            Self::Windows1258 => "windows-1258",
            Self::MacRoman => "macintosh",
            Self::Cp437 => "IBM437",
            Self::Cp850 => "IBM850",
        }
    }

    /// The canonical Windows code page identifier.
    ///
    /// Round-trips through [`Encoding::from_windows_codepage`]. ISO-8859-1
    /// reports 28591, the number Windows itself uses; 819 is accepted on the
    /// way in but is not the canonical form.
    #[must_use]
    pub const fn windows_codepage(self) -> u16 {
        match self {
            Self::Iso8859_1 => 28591,
            Self::Windows1250 => 1250,
            Self::Windows1251 => 1251,
            Self::Windows1252 => 1252,
            Self::Windows1253 => 1253,
            Self::Windows1254 => 1254,
            Self::Windows1255 => 1255,
            Self::Windows1256 => 1256,
            Self::Windows1257 => 1257,
            Self::Windows1258 => 1258,
            Self::MacRoman => 10000,
            Self::Cp437 => 437,
            Self::Cp850 => 850,
        }
    }

    /// Look up an encoding by Windows code page number.
    ///
    /// This is the lookup RTF's `\ansicpgN` and the `.lnk` / `CF_LOCALE`
    /// code page fields need: both identify an encoding numerically and neither
    /// carries a name.
    ///
    /// Returns `None` for a number that is not a single-byte code page this
    /// crate implements. That deliberately includes the ones that are perfectly
    /// real but not single-byte — 65001 (UTF-8), 1200 / 1201 (UTF-16), 932 /
    /// 936 / 949 / 950 (the DBCS pages) — because a single-byte decoder applied
    /// to those produces confident garbage rather than an error.
    #[must_use]
    pub const fn from_windows_codepage(n: u32) -> Option<Self> {
        match n {
            437 => Some(Self::Cp437),
            850 => Some(Self::Cp850),
            // 819 is IBM's number for Latin-1 and is what older RTF writers put
            // in `\ansicpg`; 28591 is the modern Windows identifier for it.
            819 | 28591 => Some(Self::Iso8859_1),
            1250 => Some(Self::Windows1250),
            1251 => Some(Self::Windows1251),
            1252 => Some(Self::Windows1252),
            1253 => Some(Self::Windows1253),
            1254 => Some(Self::Windows1254),
            1255 => Some(Self::Windows1255),
            1256 => Some(Self::Windows1256),
            1257 => Some(Self::Windows1257),
            1258 => Some(Self::Windows1258),
            10000 => Some(Self::MacRoman),
            _ => None,
        }
    }

    /// The `0x80..=0xFF` half of the mapping, or `None` when it is the identity.
    ///
    /// `None` means ISO-8859-1, whose high half is U+0080..=U+00FF and so costs
    /// no table at all. A `0` entry in the returned table marks a byte the code
    /// page leaves undefined; see [`tables`].
    #[must_use]
    pub const fn high_table(self) -> Option<&'static [u16; 128]> {
        match self {
            Self::Iso8859_1 => None,
            Self::Windows1250 => Some(&tables::WINDOWS_1250),
            Self::Windows1251 => Some(&tables::WINDOWS_1251),
            Self::Windows1252 => Some(&tables::WINDOWS_1252),
            Self::Windows1253 => Some(&tables::WINDOWS_1253),
            Self::Windows1254 => Some(&tables::WINDOWS_1254),
            Self::Windows1255 => Some(&tables::WINDOWS_1255),
            Self::Windows1256 => Some(&tables::WINDOWS_1256),
            Self::Windows1257 => Some(&tables::WINDOWS_1257),
            Self::Windows1258 => Some(&tables::WINDOWS_1258),
            Self::MacRoman => Some(&tables::MAC_ROMAN),
            Self::Cp437 => Some(&tables::CP437),
            Self::Cp850 => Some(&tables::CP850),
        }
    }

    /// Decode one byte, or `None` if this code page leaves it undefined.
    ///
    /// `None` is the whole point of this signature. Windows-1252 assigns no
    /// character to `0x81`, `0x8D`, `0x8F`, `0x90` or `0x9D`; Windows-1255
    /// assigns none to twenty-three values. Returning U+FFFD there would make
    /// "this byte means nothing in this encoding" indistinguishable from "this
    /// byte means U+FFFD", and callers that want the substitution can have it
    /// from [`Encoding::decode_byte_lossy`].
    #[must_use]
    pub const fn decode_byte(self, b: u8) -> Option<char> {
        if b < 0x80 {
            // ASCII-transparent in all thirteen. The generator proves this
            // against the upstream file for each table, so the fast path is not
            // an assumption.
            return char::from_u32(b as u32);
        }
        match self.high_table() {
            None => char::from_u32(b as u32),
            Some(table) => {
                // `b & 0x7F` equals `b - 0x80` exactly, because `b >= 0x80`
                // here. Writing it as a mask rather than a subtraction is what
                // makes the index provably `0..128` for *every* `u8`, so this
                // lookup compiles with no bounds check and no panic path.
                let cp = table[(b & 0x7F) as usize];
                if cp == 0 {
                    // Sentinel: undefined in this code page. Not U+0000, which
                    // only byte 0x00 ever maps to.
                    None
                } else {
                    char::from_u32(cp as u32)
                }
            }
        }
    }

    /// Decode one byte the way Windows itself does: an undefined value in
    /// `0x80..=0x9F` becomes the C1 control of the same number.
    ///
    /// The authoritative sources genuinely disagree here.
    /// [`Encoding::decode_byte`] follows the Unicode Consortium's vendor
    /// mapping files, which mark those bytes undefined. `MultiByteToWideChar`
    /// and the WHATWG Encoding Standard — so every browser — instead map
    /// `0x81` to U+0081, `0x8D` to U+008D and so on, for all thirty-odd such
    /// slots across the nine Windows-125x pages.
    ///
    /// Which is right depends on where the bytes came from. Text a Windows API
    /// produced went through the lenient mapping and round-trips only under
    /// this method; text from a strict transcoder never contains those bytes at
    /// all. Neither behaviour can be the silent default, so both are named.
    ///
    /// This covers only the C1 range, which is where the two sources differ
    /// mechanically. It does **not** paper over the one substantive
    /// disagreement: Windows-1255 `0xCA`, which the pinned Microsoft file
    /// leaves undefined and WHATWG maps to U+05BA. See the crate README.
    #[must_use]
    pub const fn decode_byte_lenient(self, b: u8) -> Option<char> {
        match self.decode_byte(b) {
            Some(c) => Some(c),
            None => match b {
                0x80..=0x9F => char::from_u32(b as u32),
                _ => None,
            },
        }
    }

    /// Decode one byte, substituting U+FFFD for an undefined value.
    #[must_use]
    pub const fn decode_byte_lossy(self, b: u8) -> char {
        match self.decode_byte(b) {
            Some(c) => c,
            None => char::REPLACEMENT_CHARACTER,
        }
    }

    /// `true` if this code page assigns a character to `b`.
    #[must_use]
    pub const fn is_defined(self, b: u8) -> bool {
        self.decode_byte(b).is_some()
    }

    /// `true` if some byte value is unassigned in this code page.
    ///
    /// Useful to a caller deciding whether a round-trip through this encoding
    /// can lose information at all.
    #[must_use]
    pub const fn has_undefined_bytes(self) -> bool {
        match self.high_table() {
            None => false,
            Some(table) => {
                let mut i = 0usize;
                while i < 128 {
                    if table[i] == 0 {
                        return true;
                    }
                    i += 1;
                }
                false
            }
        }
    }

    /// Encode one `char` back to a byte, or `None` if this code page has no
    /// byte for it.
    ///
    /// The reverse map is unambiguous: the generator rejects any table in which
    /// two bytes share a target, so there is exactly one answer or none. The
    /// scan is over 128 entries and is `const`, so a caller encoding a literal
    /// pays nothing at runtime.
    #[must_use]
    pub const fn encode_char(self, c: char) -> Option<u8> {
        let cp = c as u32;
        if cp < 0x80 {
            return Some(cp as u8);
        }
        match self.high_table() {
            None => {
                if cp <= 0xFF {
                    Some(cp as u8)
                } else {
                    None
                }
            }
            Some(table) => {
                if cp > 0xFFFF {
                    // Every table entry is a BMP scalar (the generator rejects
                    // anything else), so nothing above U+FFFF can match.
                    return None;
                }
                let want = cp as u16;
                let mut i = 0usize;
                while i < 128 {
                    if table[i] == want {
                        return Some((i as u8) | 0x80);
                    }
                    i += 1;
                }
                None
            }
        }
    }

    /// Decode a byte slice, one `char` per byte.
    ///
    /// Yields [`ErrorKind::Malformed`] at the offset of any byte this code page
    /// leaves undefined, and keeps going: a single-byte encoding cannot lose
    /// sync, so one bad byte says nothing about the next one.
    #[must_use]
    pub const fn decode(self, bytes: &[u8]) -> Decoder<'_> {
        Decoder::new(self, bytes)
    }

    /// Decode a byte slice, substituting U+FFFD for undefined bytes.
    #[must_use]
    pub const fn decode_lossy(self, bytes: &[u8]) -> LossyDecoder<'_> {
        LossyDecoder::new(self, bytes)
    }
}

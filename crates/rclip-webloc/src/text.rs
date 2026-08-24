//! Strings that are not `&str` yet.
//!
//! A `.webloc` stores its URL in one of three encodings and this crate refuses
//! to allocate, so the value comes back as a [`Text`] that knows which one it
//! is and can be iterated as `char`s in every case:
//!
//! - **UTF-8**, borrowed directly. XML plists that contain no entity reference,
//!   and binary plists' `0x5n` ASCII strings.
//! - **XML-escaped UTF-8.** CoreFoundation writes `&` as `&amp;`, which means
//!   *every* URL with more than one query parameter arrives escaped. Handing
//!   back the raw slice would be wrong in the most common case there is.
//! - **UTF-16 big-endian.** Binary plists' `0x6n` strings. Big-endian, unlike
//!   every Win32 structure in this workspace, so [`rclip_core::Utf16Le`] is the
//!   wrong tool and this module decodes it itself.

use rclip_core::{Error, ErrorKind, Result};

/// A borrowed string in whichever encoding the file used.
#[derive(Debug, Copy, Clone)]
pub enum Text<'a> {
    /// Plain UTF-8, usable as-is.
    Utf8(&'a str),
    /// UTF-8 that still contains XML entity references (`&amp;`, `&#38;`).
    Escaped(&'a str),
    /// UTF-16, **big-endian**, as stored in a binary plist.
    Utf16Be(&'a [u8]),
}

impl<'a> Text<'a> {
    /// The string as it stands, when no decoding is needed.
    ///
    /// `None` for the two encodings whose bytes are not the value — use
    /// [`Text::chars`], or `to_string_lossy` behind the `alloc` feature.
    /// Returning the raw slice in those cases would quietly hand back
    /// `a&amp;b` where the value is `a&b`.
    #[must_use]
    pub const fn as_str(&self) -> Option<&'a str> {
        match self {
            Self::Utf8(s) => Some(s),
            Self::Escaped(_) | Self::Utf16Be(_) => None,
        }
    }

    /// `true` when the value needs decoding before use.
    #[must_use]
    pub const fn is_encoded(&self) -> bool {
        !matches!(self, Self::Utf8(_))
    }

    /// Decode to `char`s. Fallible per character: a malformed entity reference
    /// or a lone surrogate is an error, not a substitution.
    #[must_use]
    pub const fn chars(&self) -> Chars<'a> {
        match *self {
            Self::Utf8(s) => Chars::new(s.as_bytes(), Encoding::Utf8),
            Self::Escaped(s) => Chars::new(s.as_bytes(), Encoding::Escaped),
            Self::Utf16Be(b) => Chars::new(b, Encoding::Utf16Be),
        }
    }

    /// Compare against a plain `&str` without allocating. Used for key lookup,
    /// where the key can itself be any of the three encodings.
    #[must_use]
    pub fn eq_str(&self, other: &str) -> bool {
        if let Self::Utf8(s) = self {
            return *s == other;
        }
        let mut expected = other.chars();
        for c in self.chars() {
            match (c, expected.next()) {
                (Ok(a), Some(b)) if a == b => {}
                _ => return false,
            }
        }
        expected.next().is_none()
    }

    /// Decode to an owned `String`, substituting U+FFFD for anything malformed.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn to_string_lossy(&self) -> alloc::string::String {
        if let Self::Utf8(s) = self {
            return alloc::string::String::from(*s);
        }
        let mut out = alloc::string::String::new();
        for c in self.chars() {
            out.push(c.unwrap_or('\u{FFFD}'));
        }
        out
    }
}

/// Character iterator over a [`Text`].
///
/// Fallible per character rather than up front, because the encodings that need
/// decoding can go wrong halfway: a malformed entity reference or a lone
/// surrogate is reported where it is, and iteration stops rather than
/// substituting a replacement character behind the caller's back.
#[derive(Debug, Clone)]
pub struct Chars<'a> {
    bytes: &'a [u8],
    pos: usize,
    encoding: Encoding,
}

#[derive(Debug, Copy, Clone)]
enum Encoding {
    Utf8,
    Escaped,
    Utf16Be,
}

impl<'a> Chars<'a> {
    const fn new(bytes: &'a [u8], encoding: Encoding) -> Self {
        Self { bytes, pos: 0, encoding }
    }
}

impl Iterator for Chars<'_> {
    type Item = Result<char>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.encoding {
            Encoding::Utf8 => next_utf8(self.bytes, &mut self.pos),
            Encoding::Escaped => next_escaped(self.bytes, &mut self.pos),
            Encoding::Utf16Be => next_utf16be(self.bytes, &mut self.pos),
        }
    }
}

/// Next `char` of a slice that is already known to be valid UTF-8.
fn next_utf8(bytes: &[u8], pos: &mut usize) -> Option<Result<char>> {
    let rest = bytes.get(*pos..)?;
    // Safe by construction: `Text::Utf8` only ever wraps a `&str`.
    let s = core::str::from_utf8(rest).ok()?;
    let c = s.chars().next()?;
    *pos += c.len_utf8();
    Some(Ok(c))
}

/// Next `char` of XML-escaped UTF-8, resolving one entity reference at a time.
fn next_escaped(bytes: &[u8], pos: &mut usize) -> Option<Result<char>> {
    let start = *pos;
    let first = next_utf8(bytes, pos)?;
    let Ok('&') = first else { return Some(first) };

    // Find the terminating semicolon. XML puts no upper bound on an entity
    // name, but every entity a plist can contain is short, and scanning to the
    // end of a megabyte-long URL looking for a semicolon that is not there is
    // work an attacker should not get for free.
    const MAX_ENTITY_LEN: usize = 12;
    let body_start = *pos;
    let limit = body_start.saturating_add(MAX_ENTITY_LEN).min(bytes.len());
    let window = bytes.get(body_start..limit)?;
    let Some(rel) = window.iter().position(|&b| b == b';') else {
        return Some(Err(Error::new(ErrorKind::Malformed, start)));
    };
    let name = window.get(..rel)?;
    *pos = body_start + rel + 1;

    let decoded = match name {
        b"amp" => Some('&'),
        b"lt" => Some('<'),
        b"gt" => Some('>'),
        b"quot" => Some('"'),
        b"apos" => Some('\''),
        [b'#', b'x' | b'X', digits @ ..] => parse_radix(digits, 16),
        [b'#', digits @ ..] => parse_radix(digits, 10),
        _ => None,
    };
    Some(decoded.ok_or(Error::new(ErrorKind::Malformed, start)))
}

/// Parse a numeric character reference's digits into a `char`.
///
/// Rejects an empty digit run and any code point that is not a `char` —
/// surrogates and anything above U+10FFFF included, both of which a hostile
/// file can spell perfectly legally as `&#xD800;`.
fn parse_radix(digits: &[u8], radix: u32) -> Option<char> {
    if digits.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for &d in digits {
        let n = (d as char).to_digit(radix)?;
        value = value.checked_mul(radix)?.checked_add(n)?;
    }
    char::from_u32(value)
}

/// Next `char` of big-endian UTF-16, pairing surrogates.
fn next_utf16be(bytes: &[u8], pos: &mut usize) -> Option<Result<char>> {
    let at = *pos;
    let unit = unit_be(bytes, at)?;
    *pos = at + 2;

    match unit {
        0xD800..=0xDBFF => {
            let Some(low) = unit_be(bytes, *pos) else {
                return Some(Err(Error::new(ErrorKind::InvalidUtf16, at)));
            };
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Some(Err(Error::new(ErrorKind::InvalidUtf16, at)));
            }
            *pos += 2;
            let cp = 0x1_0000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
            Some(char::from_u32(cp).ok_or(Error::new(ErrorKind::InvalidUtf16, at)))
        }
        0xDC00..=0xDFFF => Some(Err(Error::new(ErrorKind::InvalidUtf16, at))),
        _ => Some(char::from_u32(u32::from(unit)).ok_or(Error::new(ErrorKind::InvalidUtf16, at))),
    }
}

fn unit_be(bytes: &[u8], at: usize) -> Option<u16> {
    let b = bytes.get(at..at + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// `true` if the slice contains an `&`, i.e. whether it needs entity decoding.
///
/// Cheap enough to run on every value so that the common unescaped case can be
/// handed back as a plain `&str`.
pub(crate) fn has_entities(s: &str) -> bool {
    s.as_bytes().contains(&b'&')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(t: Text<'_>) -> Option<[char; 8]> {
        let mut out = ['\0'; 8];
        for (i, c) in t.chars().enumerate() {
            out[i] = c.ok()?;
        }
        Some(out)
    }

    #[test]
    fn entities_decode() {
        let t = Text::Escaped("a&amp;b&lt;c");
        let got = collect(t).unwrap();
        assert_eq!(&got[..5], &['a', '&', 'b', '<', 'c']);
    }

    #[test]
    fn numeric_entities_decode_in_both_radixes() {
        assert!(Text::Escaped("&#65;").eq_str("A"));
        assert!(Text::Escaped("&#x41;").eq_str("A"));
        assert!(Text::Escaped("&#X41;").eq_str("A"));
    }

    #[test]
    fn surrogate_escapes_are_rejected() {
        // `&#xD800;` is a well-formed entity reference for a code point that is
        // not a character. Letting it through would mean an unpaired surrogate
        // in something the caller thinks is a `char`.
        let mut it = Text::Escaped("&#xD800;").chars();
        assert!(it.next().unwrap().is_err());
    }

    #[test]
    fn unterminated_entity_does_not_scan_forever() {
        let mut it = Text::Escaped("&ampersandwithnosemicolonatallhere").chars();
        assert_eq!(it.next().unwrap().unwrap_err().kind, ErrorKind::Malformed);
    }

    #[test]
    fn utf16be_pairs_surrogates() {
        // U+1F600, as a big-endian surrogate pair.
        let bytes = [0xD8, 0x3D, 0xDE, 0x00];
        let mut it = Text::Utf16Be(&bytes).chars();
        assert_eq!(it.next().unwrap().unwrap(), '\u{1F600}');
        assert!(it.next().is_none());
    }

    #[test]
    fn lone_utf16be_surrogate_is_an_error() {
        let bytes = [0xD8, 0x3D, 0x00, 0x41];
        let mut it = Text::Utf16Be(&bytes).chars();
        assert_eq!(it.next().unwrap().unwrap_err().kind, ErrorKind::InvalidUtf16);
    }

    #[test]
    fn eq_str_works_across_encodings() {
        assert!(Text::Utf8("URL").eq_str("URL"));
        assert!(Text::Utf16Be(&[0x00, b'U', 0x00, b'R', 0x00, b'L']).eq_str("URL"));
        assert!(!Text::Utf16Be(&[0x00, b'U', 0x00, b'R']).eq_str("URL"));
        assert!(!Text::Utf8("URLName").eq_str("URL"));
    }
}

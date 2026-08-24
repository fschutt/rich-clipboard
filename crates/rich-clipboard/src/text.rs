//! Text encoding, which is a policy question on every platform.
//!
//! There is no single answer even for "plain text". `CF_UNICODETEXT` is
//! UTF-16LE and NUL-terminated; `public.utf8-plain-text` and
//! `text/plain;charset=utf-8` are UTF-8 and are not. And `text/html` is the
//! worst of the three: the flavor is nominally UTF-8, some producers write
//! UTF-16 with a BOM, some write UTF-16 without one, and there is no field
//! anywhere that says which. `plan/PLAN.md` §4.1 lists that as one of the two
//! traps to encode in the registry rather than in every caller — it is not in
//! `rclip-core` yet, so it lives here.

use alloc::string::String;
use alloc::vec::Vec;

use rclip_core::{utf16, Platform};

/// Decode a plain-text payload for `platform`.
///
/// Lossy: a lone surrogate or a bad UTF-8 sequence becomes U+FFFD rather than
/// failing the paste. A clipboard payload comes from another process, and one
/// replacement character is a better outcome than Ctrl+V doing nothing.
pub(crate) fn decode_plain(bytes: &[u8], platform: Platform) -> String {
    match platform {
        // `CF_UNICODETEXT` is UTF-16LE. Anything else on the Windows clipboard
        // is `CF_TEXT` in a code page nobody records, which `Flavor` maps to
        // `PlainText` anyway; there is nothing better to do with those bytes
        // than treat them as UTF-16 and let the replacement characters show.
        Platform::Windows => utf16::decode_utf16le_lossy(trim_wide_nuls(bytes)),
        Platform::MacOs | Platform::Unix => decode_utf8_lossy(trim_nuls(bytes)),
    }
}

/// Encode a plain-text payload for `platform`.
///
/// The Windows form is NUL-terminated, because `CF_UNICODETEXT` is defined as a
/// NUL-terminated string and a consumer that calls `lstrlenW` on it will read
/// past the end of the allocation without one.
pub(crate) fn encode_plain(text: &str, platform: Platform) -> Vec<u8> {
    match platform {
        Platform::Windows => {
            let mut out = utf16::encode_utf16le(text);
            out.extend_from_slice(&[0, 0]);
            out
        }
        Platform::MacOs | Platform::Unix => Vec::from(text.as_bytes()),
    }
}

/// Decode an HTML payload, sniffing the encoding.
///
/// BOM first, because it is the only in-band signal there is. Then the
/// interleaved-NUL check, then UTF-8 — and that order is the whole point.
/// `"<b>hi</b>"` in UTF-16LE is `3C 00 62 00 3E 00 …`, which is **valid UTF-8**:
/// NUL is a perfectly good UTF-8 character. A reader that tries UTF-8 first and
/// falls back to UTF-16 on failure therefore never falls back at all, and hands
/// the caller a string with a NUL between every letter.
///
/// A legacy code page is never guessed at. A wrong guess produces mojibake that
/// looks like real text and survives into the user's document; a replacement
/// character is at least visibly a gap.
///
/// # Limits
///
/// UTF-16 without a BOM is only detectable by the NULs, so a UTF-16 payload
/// that is mostly non-ASCII — CJK markup — will not be recognised. Producers
/// that write UTF-16 HTML write the BOM, and this is why.
#[cfg(feature = "html")]
pub(crate) fn decode_html_bytes(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return decode_utf8_lossy(trim_nuls(rest));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16::decode_utf16le_lossy(trim_wide_nuls(rest));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16be_lossy(rest);
    }
    if looks_utf16le(bytes) {
        return utf16::decode_utf16le_lossy(trim_wide_nuls(bytes));
    }
    decode_utf8_lossy(trim_nuls(bytes))
}

/// Heuristic: at least a third of the even-indexed pairs have a zero high byte.
#[cfg(feature = "html")]
fn looks_utf16le(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || bytes.len() % 2 != 0 {
        return false;
    }
    let pairs = bytes.len() / 2;
    let zeros = bytes.chunks_exact(2).filter(|p| p[1] == 0).count();
    zeros * 3 >= pairs
}

#[cfg(feature = "html")]
fn decode_utf16be_lossy(bytes: &[u8]) -> String {
    let mut swapped = Vec::with_capacity(bytes.len());
    for pair in bytes.chunks_exact(2) {
        swapped.push(pair[1]);
        swapped.push(pair[0]);
    }
    utf16::decode_utf16le_lossy(trim_wide_nuls(&swapped))
}

/// `String::from_utf8_lossy` without `std`, and without a copy when it is
/// already valid.
pub(crate) fn decode_utf8_lossy(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut rest = bytes;
    loop {
        match core::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // Safe by construction: `valid_up_to` is a character boundary.
                out.push_str(core::str::from_utf8(&rest[..valid]).unwrap_or_default());
                out.push(char::REPLACEMENT_CHARACTER);
                match e.error_len() {
                    Some(len) => rest = &rest[valid + len..],
                    // An incomplete sequence at the end of the input. Nothing
                    // follows it, so one replacement character is the whole
                    // remainder.
                    None => return out,
                }
            }
        }
    }
}

/// Drop trailing NUL bytes. `CF_UNICODETEXT` and `CF_RTF` both arrive
/// NUL-terminated off the Windows clipboard; so, per Qt's own source comment,
/// does `text/uri-list` from a Qt 3 application.
fn trim_nuls(mut bytes: &[u8]) -> &[u8] {
    while let Some(rest) = bytes.strip_suffix(&[0]) {
        bytes = rest;
    }
    bytes
}

/// Drop trailing UTF-16 NUL units, and an odd trailing byte if the buffer has
/// one — a half code unit cannot be decoded and is never content.
fn trim_wide_nuls(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len() - (bytes.len() % 2);
    while end >= 2 && bytes[end - 1] == 0 && bytes[end - 2] == 0 {
        end -= 2;
    }
    &bytes[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_plain_text_round_trips_with_its_terminator() {
        let bytes = encode_plain("hi", Platform::Windows);
        assert_eq!(bytes, [b'h', 0, b'i', 0, 0, 0]);
        assert_eq!(decode_plain(&bytes, Platform::Windows), "hi");
    }

    #[cfg(feature = "html")]
    #[test]
    fn utf16le_html_without_a_bom_is_still_read() {
        let utf16: Vec<u8> = "<b>hi</b>"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_html_bytes(&utf16), "<b>hi</b>");
    }

    #[cfg(feature = "html")]
    #[test]
    fn a_single_bad_utf8_byte_does_not_trigger_the_utf16_fallback() {
        // If this fell through to UTF-16 the result would be CJK noise rather
        // than the markup with one gap in it.
        let mut bytes = Vec::from(&b"<b>caf\xC3\xA9</b>"[..]);
        bytes[6] = 0xFF;
        let out = decode_html_bytes(&bytes);
        assert!(out.starts_with("<b>caf"), "{out}");
        assert!(out.ends_with("</b>"), "{out}");
    }
}

//! The `text/html` encoding trap, which `plan/PLAN.md` §4.1 names as one of
//! the two things the registry has to absorb so no caller re-derives it.

#![cfg(feature = "alloc")]

use rclip_core::{decode_html_bytes, decode_plain, encode_plain, Platform};

#[test]
fn windows_plain_text_round_trips_with_its_terminator() {
    // CF_UNICODETEXT is defined as NUL-terminated, and a consumer calling
    // lstrlenW on a buffer without one reads past the allocation.
    let bytes = encode_plain("hi", Platform::Windows);
    assert_eq!(bytes, [b'h', 0, b'i', 0, 0, 0]);
    assert_eq!(decode_plain(&bytes, Platform::Windows), "hi");
}

#[test]
fn the_other_platforms_are_utf8_and_unterminated() {
    for p in [Platform::MacOs, Platform::Unix] {
        assert_eq!(encode_plain("hi", p), b"hi".to_vec());
        assert_eq!(decode_plain(b"hi", p), "hi");
    }
}

#[test]
fn utf16le_html_is_valid_utf8_which_is_why_the_order_matters() {
    // "<b>hi</b>" as UTF-16LE is 3C 00 62 00 ... — every one of those bytes is
    // a legal UTF-8 character, NUL included. A reader that tries UTF-8 first
    // and only falls back on failure never falls back, and hands the caller a
    // string with a NUL between every letter. This test is the regression.
    let utf16: Vec<u8> = "<b>hi</b>"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert!(
        core::str::from_utf8(&utf16).is_ok(),
        "the premise: these bytes really are valid UTF-8"
    );
    assert_eq!(decode_html_bytes(&utf16), "<b>hi</b>");
}

#[test]
fn every_bom_is_honoured_over_the_heuristic() {
    let mut utf8 = vec![0xEF, 0xBB, 0xBF];
    utf8.extend_from_slice("<b>é</b>".as_bytes());
    assert_eq!(decode_html_bytes(&utf8), "<b>é</b>");

    let mut le = vec![0xFF, 0xFE];
    le.extend("<b>é</b>".encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(decode_html_bytes(&le), "<b>é</b>");

    let mut be = vec![0xFE, 0xFF];
    be.extend("<b>é</b>".encode_utf16().flat_map(u16::to_be_bytes));
    assert_eq!(decode_html_bytes(&be), "<b>é</b>");
}

#[test]
fn one_bad_byte_does_not_trigger_the_utf16_fallback() {
    // The failure mode this guards: falling through to UTF-16 on any UTF-8
    // error turns a document with one corrupt byte into CJK noise.
    let mut bytes = Vec::from(&b"<b>caf\xC3\xA9</b>"[..]);
    bytes[6] = 0xFF;
    let out = decode_html_bytes(&bytes);
    assert!(out.starts_with("<b>caf"), "{out}");
    assert!(out.ends_with("</b>"), "{out}");
}

#[test]
fn a_legacy_code_page_is_never_guessed() {
    // 0xE9 is é in Windows-1252 and invalid UTF-8. Guessing would produce text
    // that looks right and survives into the user's document; a replacement
    // character is at least visibly a gap.
    let out = decode_html_bytes(b"<b>caf\xE9</b>");
    assert!(
        out.contains('\u{FFFD}'),
        "must not silently become 'café': {out}"
    );
}

#[test]
fn empty_and_odd_length_inputs_do_not_panic() {
    assert_eq!(decode_html_bytes(b""), "");
    assert_eq!(
        decode_html_bytes(&[0xFF, 0xFE]),
        "",
        "a BOM and nothing else"
    );
    // An odd trailing byte is half a UTF-16 code unit. It is dropped rather
    // than turned into U+FFFD, because it is not a *corrupt* character — it is
    // an incomplete one, and the byte that would complete it may simply not
    // have been transferred yet. A lone byte is therefore an empty string, not
    // a replacement character.
    let _ = decode_html_bytes(&[0xFF, 0xFE, 0x3C, 0x00, 0x62]);
    assert_eq!(decode_plain(&[0x41], Platform::Windows), "");
    assert_eq!(
        decode_plain(&[b'h', 0, b'i', 0, 0x21], Platform::Windows),
        "hi",
        "the complete units survive; only the half unit goes"
    );
}

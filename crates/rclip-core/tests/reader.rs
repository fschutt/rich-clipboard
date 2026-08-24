//! The reader is the safety boundary for every codec in the workspace, so its
//! failure modes are tested harder than its happy paths.

use rclip_core::{
    error::ErrorKind,
    utf16::{is_valid_utf16le, utf16le_char_count, Utf16Le},
    Reader,
};

#[test]
fn reads_little_endian_integers() {
    let buf = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let mut r = Reader::new(&buf);
    assert_eq!(r.u8().unwrap(), 0x01);
    assert_eq!(r.u16_le().unwrap(), 0x0302);
    assert_eq!(r.u32_le().unwrap(), 0x0706_0504);
    assert_eq!(r.remaining_len(), 1);
}

#[test]
fn take_past_end_reports_the_offset_it_failed_at() {
    let buf = [0u8; 4];
    let mut r = Reader::new(&buf);
    r.skip(3).unwrap();
    let err = r.take(4).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    assert_eq!(err.offset, 3, "offset must be where the read started");
}

#[test]
fn take_does_not_advance_on_failure() {
    let buf = [1u8, 2, 3];
    let mut r = Reader::new(&buf);
    assert!(r.take(99).is_err());
    assert_eq!(r.pos(), 0, "a failed read must leave the cursor put");
    assert_eq!(r.take(3).unwrap(), &[1, 2, 3]);
}

#[test]
fn overflowing_length_is_too_large_not_a_panic() {
    let buf = [0u8; 8];
    let mut r = Reader::new(&buf);
    r.skip(4).unwrap();
    let err = r.take(usize::MAX).unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooLarge);
}

#[test]
fn seek_rejects_out_of_range_rather_than_clamping() {
    let buf = [0u8; 4];
    let mut r = Reader::new(&buf);
    assert!(r.seek(4).is_ok(), "seeking to exactly the end is valid");
    let err = r.seek(5).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadOffset);
    assert_eq!(err.offset, 5);
}

#[test]
fn check_count_rejects_a_count_the_input_cannot_back() {
    let buf = [0u8; 16];
    let r = Reader::new(&buf);
    assert!(r.check_count(4, 4).is_ok());
    assert_eq!(r.check_count(5, 4).unwrap_err().kind, ErrorKind::TooLarge);
    assert_eq!(
        r.check_count(0xFFFF_FFFF, 4).unwrap_err().kind,
        ErrorKind::TooLarge
    );
}

#[test]
fn take_reader_confines_an_inner_parser_to_its_record() {
    let buf = [1u8, 2, 3, 4, 5, 6];
    let mut outer = Reader::new(&buf);
    let mut inner = outer.take_reader(3).unwrap();
    assert_eq!(inner.take(3).unwrap(), &[1, 2, 3]);
    assert!(inner.u8().is_err(), "inner must not see the outer's tail");
    assert_eq!(outer.pos(), 3);
}

#[test]
fn cstr_requires_a_terminator() {
    let mut r = Reader::new(b"abc\0def\0");
    assert_eq!(r.cstr_utf8().unwrap(), "abc");
    assert_eq!(r.cstr_utf8().unwrap(), "def");

    let mut unterminated = Reader::new(b"abc");
    assert_eq!(
        unterminated.cstr_bytes().unwrap_err().kind,
        ErrorKind::UnexpectedEof,
        "an unterminated string must fail, not return the tail"
    );
}

#[test]
fn utf16_fixed_truncates_at_nul_but_consumes_the_whole_field() {
    let mut buf = [0u8; 16];
    buf[0..4].copy_from_slice(&[b'H', 0, b'i', 0]);
    let mut r = Reader::new(&buf);
    let name = r.utf16_fixed(8).unwrap();
    assert_eq!(name, &[b'H', 0, b'i', 0]);
    assert_eq!(r.pos(), 16, "the full fixed field must be consumed");
}

#[test]
fn f64_be_is_the_bookmark_date_case() {
    let bytes = 1.0f64.to_be_bytes();
    let mut r = Reader::new(&bytes);
    assert_eq!(r.f64_be().unwrap(), 1.0);
}

#[test]
fn utf16_decodes_surrogate_pairs() {
    let bytes = [0x3E, 0xD8, 0x80, 0xDD];
    let chars: Vec<char> = Utf16Le::new(&bytes).map(Result::unwrap).collect();
    assert_eq!(chars, vec!['\u{1F980}']);
    assert_eq!(utf16le_char_count(&bytes), Some(1));
}

#[test]
fn utf16_reports_lone_surrogates_instead_of_substituting() {
    let lone_high = [0x3D, 0xD8];
    assert!(!is_valid_utf16le(&lone_high));
    assert!(Utf16Le::new(&lone_high).next().unwrap().is_err());

    let lone_low = [0x80, 0xDD];
    assert!(!is_valid_utf16le(&lone_low));

    let odd_length = [0x41, 0x00, 0x42];
    assert!(!is_valid_utf16le(&odd_length));
}

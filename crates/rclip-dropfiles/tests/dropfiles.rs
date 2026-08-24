//! `CF_HDROP` against the synthetic corpus.
//!
//! The malformed fixtures matter most: this format's `pFiles` offset and its
//! double-NUL terminator are both attacker-controlled, and both have to fail
//! with a specific `ErrorKind` rather than panic.

use rclip_core::ErrorKind;
use rclip_dropfiles::{to_bytes, Builder, DropFiles, Path, Point, HEADER_LEN};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/");
    std::fs::read(format!("{p}rclip-dropfiles/{name}")).expect("fixture")
}

/// The `.bin` files and what their sidecars say to expect.
fn sidecar_expectations() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/rclip-dropfiles");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let json = std::fs::read_to_string(&path).expect("sidecar");
        let expect = json
            .split("\"expect\"")
            .nth(1)
            .and_then(|s| s.split('"').nth(1))
            .expect("sidecar has an \"expect\" field")
            .to_owned();
        let stem = path.file_stem().unwrap().to_str().unwrap().to_owned();
        out.push((format!("{stem}.bin"), expect));
    }
    out.sort();
    out
}

fn wide(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[test]
fn every_fixture_matches_its_sidecar() {
    let cases = sidecar_expectations();
    assert!(cases.len() >= 6, "corpus should not silently shrink");
    for (name, expect) in cases {
        let bytes = fixture(&name);
        let parsed = DropFiles::parse(&bytes);
        match expect.as_str() {
            "ok" => {
                parsed.unwrap_or_else(|e| panic!("{name} is declared ok but failed: {e}"));
            }
            "error" => {
                assert!(parsed.is_err(), "{name} is declared error but parsed cleanly");
            }
            other => panic!("{name}: unknown expect value {other:?}"),
        }
    }
}

#[test]
fn two_wide_paths_decode_including_non_ascii() {
    let bytes = fixture("two-paths-wide.bin");
    let drop = DropFiles::parse(&bytes).unwrap();

    assert!(drop.is_wide());
    assert!(!drop.is_non_client());
    assert_eq!(drop.point(), Point::ORIGIN);
    assert_eq!(drop.list_offset(), HEADER_LEN);
    assert_eq!(drop.count(), 2, "the array terminator must not count as a path");

    let names: Vec<String> = drop.paths().map(|p| p.to_string_lossy().unwrap()).collect();
    assert_eq!(names, ["C:\\Users\\felix\\report.pdf", "C:\\Users\\felix\\Bilder\\föö.png"]);
}

#[test]
fn a_single_path_is_not_swallowed_by_the_terminator() {
    // The off-by-one that eats the only path in a one-element list.
    let bytes = fixture("single-path-wide.bin");
    let drop = DropFiles::parse(&bytes).unwrap();
    let names: Vec<String> = drop.paths().map(|p| p.to_string_lossy().unwrap()).collect();
    assert_eq!(names, ["C:\\a.txt"]);
}

#[test]
fn an_empty_list_is_zero_paths_not_one_empty_path() {
    let bytes = fixture("empty.bin");
    let drop = DropFiles::parse(&bytes).unwrap();
    assert!(drop.is_empty());
    assert_eq!(drop.count(), 0, "a lone terminator must not decode as an empty filename");
    assert_eq!(drop.paths().next(), None);
}

#[test]
fn drop_point_and_nonclient_flag_survive() {
    let bytes = fixture("drop-point-nonclient.bin");
    let drop = DropFiles::parse(&bytes).unwrap();
    assert_eq!(drop.point(), Point::new(1280, 720));
    assert!(drop.is_non_client(), "fNC says pt is in screen coordinates");
}

#[test]
fn p_files_is_honoured_rather_than_assumed_to_be_twenty() {
    let bytes = fixture("offset-padded.bin");
    let drop = DropFiles::parse(&bytes).unwrap();
    assert_eq!(drop.list_offset(), 24, "the fixture puts a four-byte gap after the header");
    let names: Vec<String> = drop.paths().map(|p| p.to_string_lossy().unwrap()).collect();
    assert_eq!(names, ["C:\\padded.txt"], "hardcoding 20 would read the padding as text");
}

#[test]
fn ansi_paths_come_back_as_raw_bytes() {
    let bytes = fixture("ansi-two-paths.bin");
    let drop = DropFiles::parse(&bytes).unwrap();
    assert!(!drop.is_wide());

    let paths: Vec<Path<'_>> = drop.paths().collect();
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0], Path::Ansi(b"C:\\temp1.txt"));
    assert_eq!(paths[1], Path::Ansi(b"C:\\temp2.txt"));
    assert!(!paths[0].is_wide());
    assert!(paths[0].chars().is_none(), "an ANSI path has no codepage to decode with");
    assert_eq!(paths[0].to_string_lossy(), None, "guessing a codepage is out of scope");
}

#[test]
fn p_files_past_the_end_is_a_bad_offset_not_a_panic() {
    let bytes = fixture("bad-offset.bin");
    let err = DropFiles::parse(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadOffset, "pFiles=65536 in a 40-byte buffer");
}

#[test]
fn a_missing_array_terminator_is_eof_not_a_silent_truncation() {
    let bytes = fixture("unterminated-list.bin");
    let err = DropFiles::parse(&bytes).unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::UnexpectedEof,
        "without the extra NUL there is no way to know the list ended"
    );
}

#[test]
fn p_files_pointing_into_the_header_is_rejected() {
    // Legal-looking (inside the buffer) but nonsense: the "paths" would be the
    // header's own DWORDs.
    let mut bytes = fixture("single-path-wide.bin");
    bytes[..4].copy_from_slice(&8u32.to_le_bytes());
    let err = DropFiles::parse(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadOffset, "pFiles below 20 aliases the header");
}

#[test]
fn a_truncated_header_is_eof() {
    for len in 0..HEADER_LEN {
        let bytes = fixture("single-path-wide.bin");
        let err = DropFiles::parse(&bytes[..len]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof, "{len}-byte header must not panic");
    }
}

#[test]
fn a_nonstandard_true_still_reads_as_true() {
    // Win32 BOOL is an int; sources have shipped -1 for TRUE.
    let mut bytes = fixture("single-path-wide.bin");
    bytes[16..20].copy_from_slice(&(-1i32).to_le_bytes());
    let drop = DropFiles::parse(&bytes).unwrap();
    assert!(drop.is_wide(), "any nonzero fWide is TRUE, not just 1");
}

#[test]
fn trailing_padding_after_the_terminator_is_ignored() {
    // GlobalAlloc rounds up, so a real payload is usually longer than its
    // contents. Those extra zeros must not read as extra empty paths.
    let mut bytes = fixture("single-path-wide.bin");
    bytes.extend_from_slice(&[0u8; 16]);
    let drop = DropFiles::parse(&bytes).unwrap();
    assert_eq!(drop.count(), 1);
}

// ---------------------------------------------------------------------------
// Serializing
// ---------------------------------------------------------------------------

#[test]
fn canonical_fixtures_round_trip_byte_for_byte() {
    for name in ["two-paths-wide.bin", "single-path-wide.bin", "empty.bin", "ansi-two-paths.bin"] {
        let bytes = fixture(name);
        let drop = DropFiles::parse(&bytes).unwrap();
        assert_eq!(drop.to_bytes(), bytes, "{name} should re-serialize identically");
    }
}

#[test]
fn a_padded_payload_round_trips_semantically_not_byte_wise() {
    let bytes = fixture("offset-padded.bin");
    let drop = DropFiles::parse(&bytes).unwrap();
    let round = drop.to_bytes();
    assert_ne!(round, bytes, "the gap is dropped in favour of the canonical pFiles=20");
    let again = DropFiles::parse(&round).unwrap();
    assert_eq!(again.list_offset(), HEADER_LEN);
    assert_eq!(again.raw_list(), drop.raw_list());
}

#[test]
fn a_built_payload_parses_back() {
    let mut b = Builder::wide().at(Point::new(7, 9)).non_client(true);
    b.push_str("C:\\a.txt").unwrap();
    b.push_str("D:\\Ordner\\ünïcode.txt").unwrap();
    let bytes = b.finish();

    let drop = DropFiles::parse(&bytes).unwrap();
    assert_eq!(drop.point(), Point::new(7, 9));
    assert!(drop.is_non_client());
    assert!(drop.is_wide());
    let names: Vec<String> = drop.paths().map(|p| p.to_string_lossy().unwrap()).collect();
    assert_eq!(names, ["C:\\a.txt", "D:\\Ordner\\ünïcode.txt"]);
}

#[test]
fn parsed_paths_can_be_pushed_straight_back() {
    let bytes = fixture("two-paths-wide.bin");
    let drop = DropFiles::parse(&bytes).unwrap();
    let mut b = Builder::wide();
    for p in drop.paths() {
        b.push(p).unwrap();
    }
    assert_eq!(b.finish(), bytes);
}

#[test]
fn an_empty_builder_emits_a_valid_zero_file_list() {
    let bytes = Builder::wide().finish();
    assert_eq!(bytes.len(), HEADER_LEN + 2, "header plus one wide NUL unit");
    let drop = DropFiles::parse(&bytes).unwrap();
    assert_eq!(drop.count(), 0);
}

#[test]
fn an_empty_ansi_builder_emits_one_byte_of_terminator() {
    let bytes = Builder::ansi().finish();
    assert_eq!(bytes.len(), HEADER_LEN + 1);
    let drop = DropFiles::parse(&bytes).unwrap();
    assert!(!drop.is_wide());
    assert_eq!(drop.count(), 0);
}

#[test]
fn an_embedded_nul_is_refused_rather_than_truncating_the_list() {
    // Accepting this would append a bogus extra path and, for the last entry,
    // terminate the whole array early.
    let mut b = Builder::wide();
    let err = b.push_str("C:\\a\0b.txt").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);

    let err = b.push(Path::Wide(&wide("a\0b"))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);

    let mut a = Builder::ansi();
    let err = a.push(Path::Ansi(b"a\0b")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn the_builder_refuses_a_path_in_the_wrong_encoding() {
    let mut b = Builder::wide();
    assert_eq!(b.push(Path::Ansi(b"C:\\a.txt")).unwrap_err().kind, ErrorKind::Unsupported);

    let mut a = Builder::ansi();
    assert_eq!(a.push(Path::Wide(&wide("C:\\a.txt"))).unwrap_err().kind, ErrorKind::Unsupported);
    assert_eq!(a.push_str("C:\\a.txt").unwrap_err().kind, ErrorKind::Unsupported);
}

#[test]
fn an_odd_length_wide_path_is_a_bad_length() {
    let mut b = Builder::wide();
    let err = b.push(Path::Wide(b"abc")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLength, "UTF-16 comes in two-byte units");
}

#[test]
fn to_bytes_is_the_one_liner_it_claims_to_be() {
    let bytes = to_bytes(["C:\\a.txt", "C:\\b.txt"]).unwrap();
    let drop = DropFiles::parse(&bytes).unwrap();
    assert_eq!(drop.point(), Point::ORIGIN);
    assert_eq!(drop.count(), 2);
}

#[test]
fn every_prefix_of_every_fixture_either_parses_or_errors() {
    // A cheap stand-in for a fuzzer: no truncation of any fixture may panic.
    for (name, _) in sidecar_expectations() {
        let bytes = fixture(&name);
        for len in 0..=bytes.len() {
            let _ = DropFiles::parse(&bytes[..len]);
        }
    }
}

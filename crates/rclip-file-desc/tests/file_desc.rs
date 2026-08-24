//! `CFSTR_FILEDESCRIPTORW` against the synthetic corpus.
//!
//! Two things are being pinned down here. First, that `cItems` is checked
//! against the buffer before anything iterates on it — that field is a `u32`
//! from another process. Second, that a clear flag reads as `None` and never as
//! the plausible-looking zero (or, in the corpus, `0xDEADBEEF`) sitting in the
//! field.

use rclip_core::ErrorKind;
use rclip_file_desc::{
    file_attribute, Builder, FileDescriptor, FileGroupDescriptor, Flags, PointL, RawDescriptor,
    SizeL, DESCRIPTOR_LEN, FILE_NAME_UNITS, GROUP_HEADER_LEN, MAX_WRITABLE_NAME_UNITS,
};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/");
    std::fs::read(format!("{p}rclip-file-desc/{name}")).expect("fixture")
}

fn sidecar_expectations() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/rclip-file-desc");
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

#[test]
fn the_documented_struct_sizes_are_what_we_encode() {
    // 4 + 16 + 8 + 8 + 4 + 8 + 8 + 8 + 4 + 4 + 520. Every member is 4-byte
    // aligned, so there is no padding to account for and FILEGROUPDESCRIPTORW
    // is exactly 596 bytes.
    assert_eq!(DESCRIPTOR_LEN, 592);
    assert_eq!(GROUP_HEADER_LEN + DESCRIPTOR_LEN, 596);
    assert_eq!(FILE_NAME_UNITS, 260, "cFileName is WCHAR[MAX_PATH]");
    assert_eq!(MAX_WRITABLE_NAME_UNITS, 259, "one unit is reserved for the NUL");
}

#[test]
fn flag_values_match_the_documented_constants() {
    assert_eq!(Flags::CLSID.bits(), 0x0000_0001);
    assert_eq!(Flags::SIZEPOINT.bits(), 0x0000_0002);
    assert_eq!(Flags::ATTRIBUTES.bits(), 0x0000_0004);
    assert_eq!(Flags::CREATETIME.bits(), 0x0000_0008);
    assert_eq!(Flags::ACCESSTIME.bits(), 0x0000_0010);
    assert_eq!(Flags::WRITESTIME.bits(), 0x0000_0020);
    assert_eq!(Flags::FILESIZE.bits(), 0x0000_0040);
    assert_eq!(Flags::PROGRESSUI.bits(), 0x0000_4000);
    assert_eq!(Flags::LINKUI.bits(), 0x0000_8000);
    assert_eq!(Flags::UNICODE.bits(), 0x8000_0000);
}

#[test]
fn unknown_flag_bits_are_preserved_not_rejected() {
    // dwFlags is a DWORD and Microsoft has added bits before; an unrecognised
    // one is not grounds for failing the parse.
    let f = Flags::from_bits(0x0000_0040 | 0x0000_0200);
    assert!(f.contains(Flags::FILESIZE));
    assert_eq!(f.unknown_bits(), 0x0000_0200);
    assert_eq!(f.bits(), 0x0000_0240);
}

#[test]
fn flags_debug_names_the_bits_it_knows() {
    let shown = format!("{:?}", Flags::FILESIZE | Flags::LINKUI);
    assert_eq!(shown, "Flags(FILESIZE|LINKUI)");
    assert_eq!(format!("{:?}", Flags::NONE), "Flags(NONE)");
}

#[test]
fn every_fixture_matches_its_sidecar() {
    let cases = sidecar_expectations();
    assert!(cases.len() >= 4, "corpus should not silently shrink");
    for (name, expect) in cases {
        let bytes = fixture(&name);
        let parsed = FileGroupDescriptor::parse(&bytes);
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
fn two_descriptors_with_different_flag_sets() {
    let bytes = fixture("two-descriptors.bin");
    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    assert_eq!(group.len(), 2);
    assert!(!group.is_empty());

    let first = group.get(0).unwrap();
    assert_eq!(first.file_name_lossy(), "report.pdf");
    // 4 GiB + 32: proves the high DWORD is not being dropped.
    assert_eq!(first.file_size(), Some(0x0000_0001_0000_0020));
    assert_eq!(first.file_attributes(), Some(file_attribute::NORMAL));
    assert_eq!(first.last_write_time(), Some(129_383_136_000_000_000));
    assert!(first.wants_progress_ui());
    assert!(first.is_unicode());
    assert!(!first.is_shortcut());
    assert!(!first.is_directory());
    // Flags this descriptor does not set.
    assert_eq!(first.creation_time(), None);
    assert_eq!(first.last_access_time(), None);
    assert_eq!(first.clsid(), None);
    assert_eq!(first.icon_size(), None);

    let second = group.get(1).unwrap();
    assert_eq!(second.file_name_lossy(), "Anhänge");
    assert_eq!(second.clsid(), Some([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]));
    assert_eq!(second.icon_size(), Some(SizeL { cx: 32, cy: 32 }));
    assert_eq!(second.icon_position(), Some(PointL { x: -4, y: 9 }));
    assert_eq!(second.file_attributes(), Some(file_attribute::DIRECTORY));
    assert!(second.is_directory());
    assert!(second.is_shortcut(), "FD_LINKUI is set");
    assert!(!second.wants_progress_ui());
}

#[test]
fn a_field_whose_flag_is_clear_reads_as_none_not_as_its_bytes() {
    // The second fixture descriptor holds 0xDEADBEEF in nFileSizeHigh/Low with
    // FD_FILESIZE clear. A parser that ignores dwFlags reports a 16-exabyte
    // directory; this one reports "not stated".
    let bytes = fixture("two-descriptors.bin");
    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    let second = group.get(1).unwrap();

    assert_eq!(second.file_size(), None, "FD_FILESIZE is clear");
    assert_eq!(second.raw().file_size, 0xDEAD_BEEF, "but the bytes are still there");
}

#[test]
fn a_zero_length_file_is_some_zero_not_none() {
    // Per the docs, FD_FILESIZE with both halves zero is how you promise an
    // empty file. Conflating that with "size unknown" makes the two
    // indistinguishable.
    let bytes = fixture("zero-length-file.bin");
    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    let d = group.get(0).unwrap();
    assert_eq!(d.file_size(), Some(0));
    assert_eq!(d.file_name_lossy(), "empty.txt");
}

#[test]
fn an_empty_group_is_legal() {
    let bytes = fixture("empty-group.bin");
    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    assert!(group.is_empty());
    assert_eq!(group.len(), 0);
    assert_eq!(group.iter().count(), 0);
}

#[test]
fn a_huge_c_items_is_too_large_not_an_allocation() {
    let bytes = fixture("huge-count.bin");
    let err = FileGroupDescriptor::parse(&bytes).unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::TooLarge,
        "cItems x 592 must be checked against the buffer before anything iterates"
    );
}

#[test]
fn a_descriptor_short_of_its_592_bytes_fails_at_the_count() {
    let bytes = fixture("truncated-descriptor.bin");
    let err = FileGroupDescriptor::parse(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooLarge);
}

#[test]
fn a_missing_c_items_word_is_eof() {
    for len in 0..GROUP_HEADER_LEN {
        let err = FileGroupDescriptor::parse(&[0u8; 4][..len]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnexpectedEof, "{len}-byte payload must not panic");
    }
}

#[test]
fn trailing_bytes_past_the_last_descriptor_are_ignored() {
    // The payload arrives in an HGLOBAL and GlobalAlloc rounds capacity up.
    let mut bytes = fixture("zero-length-file.bin");
    bytes.extend_from_slice(&[0xAA; 32]);
    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    assert_eq!(group.len(), 1);
    assert_eq!(group.raw_items().len(), DESCRIPTOR_LEN);
}

#[test]
fn the_name_stops_at_its_nul_and_not_at_the_end_of_the_field() {
    let bytes = fixture("zero-length-file.bin");
    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    let d = group.get(0).unwrap();
    assert_eq!(d.file_name_utf16().len(), "empty.txt".len() * 2);
    assert_eq!(d.file_name_chars().map(Result::unwrap).collect::<String>(), "empty.txt");
}

#[test]
fn a_name_that_fills_the_field_without_a_nul_still_reads() {
    // Sloppy producers exist; 260 units with no terminator must not run past
    // the field or return the following descriptor's bytes.
    let long = "x".repeat(FILE_NAME_UNITS);
    let mut raw = vec![0u8; DESCRIPTOR_LEN];
    for (i, unit) in long.encode_utf16().enumerate() {
        raw[72 + i * 2..72 + i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
    }
    let d = FileDescriptor::parse(&raw).unwrap();
    assert_eq!(d.file_name_lossy(), long);
}

#[test]
fn a_descriptor_of_the_wrong_length_is_a_bad_length() {
    assert_eq!(
        FileDescriptor::parse(&[0u8; DESCRIPTOR_LEN - 1]).unwrap_err().kind,
        ErrorKind::BadLength
    );
    assert_eq!(
        FileDescriptor::parse(&[0u8; DESCRIPTOR_LEN + 1]).unwrap_err().kind,
        ErrorKind::BadLength
    );
}

#[test]
fn get_is_bounded_by_the_declared_count() {
    let bytes = fixture("two-descriptors.bin");
    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    assert!(group.get(1).is_some());
    assert!(group.get(2).is_none());
    assert!(group.get(usize::MAX).is_none(), "must not overflow when scaling the index");
}

#[test]
fn iter_and_get_agree() {
    let bytes = fixture("two-descriptors.bin");
    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    let by_iter: Vec<_> = group.iter().collect();
    let by_index: Vec<_> = (0..group.len()).map(|i| group.get(i).unwrap()).collect();
    assert_eq!(by_iter, by_index);
    assert_eq!(group.iter().len(), 2, "size_hint must be exact");
    assert_eq!((&group).into_iter().count(), 2);
}

// ---------------------------------------------------------------------------
// Serializing
// ---------------------------------------------------------------------------

#[test]
fn fixtures_round_trip_byte_for_byte() {
    for name in ["two-descriptors.bin", "zero-length-file.bin", "empty-group.bin"] {
        let bytes = fixture(name);
        let group = FileGroupDescriptor::parse(&bytes).unwrap();
        let mut b = Builder::new();
        for d in group.iter() {
            b.push_descriptor(&d).unwrap();
        }
        assert_eq!(b.finish(), bytes, "{name} should re-serialize identically");
    }
}

#[test]
fn a_built_descriptor_parses_back_with_the_flags_its_setters_implied() {
    let mut b = Builder::new();
    b.push(
        RawDescriptor::new()
            .with_file_size(4096)
            .with_attributes(file_attribute::NORMAL)
            .with_last_write_time(129_383_136_000_000_000)
            .with_progress_ui(),
        "generated.pdf",
    )
    .unwrap();
    b.push(
        RawDescriptor::new()
            .with_attributes(file_attribute::DIRECTORY)
            .with_icon(SizeL { cx: 16, cy: 16 }, PointL { x: 1, y: 2 })
            .with_clsid([0xAB; 16])
            .with_shortcut()
            .with_unicode(),
        "Ordner\\ünïcode.txt",
    )
    .unwrap();
    assert_eq!(b.len(), 2);
    let bytes = b.finish();
    assert_eq!(bytes.len(), GROUP_HEADER_LEN + 2 * DESCRIPTOR_LEN);

    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    let first = group.get(0).unwrap();
    assert_eq!(first.file_size(), Some(4096));
    assert_eq!(first.file_attributes(), Some(file_attribute::NORMAL));
    assert_eq!(first.last_write_time(), Some(129_383_136_000_000_000));
    assert!(first.wants_progress_ui());
    assert_eq!(first.file_name_lossy(), "generated.pdf");

    let second = group.get(1).unwrap();
    assert_eq!(second.icon_size(), Some(SizeL { cx: 16, cy: 16 }));
    assert_eq!(second.icon_position(), Some(PointL { x: 1, y: 2 }));
    assert_eq!(second.clsid(), Some([0xAB; 16]));
    assert!(second.is_shortcut());
    assert!(second.is_unicode());
    assert!(second.is_directory());
    assert_eq!(second.file_name_lossy(), "Ordner\\ünïcode.txt");
}

#[test]
fn setters_set_the_flag_that_makes_their_field_mean_anything() {
    // Writing a size without FD_FILESIZE is the standard way to produce a
    // descriptor Explorer renders as zero bytes.
    let raw = RawDescriptor::new().with_file_size(1);
    assert!(raw.flags.contains(Flags::FILESIZE));
    assert!(RawDescriptor::new().with_icon(SizeL::default(), PointL::default()).flags
        .contains(Flags::SIZEPOINT));
    assert!(RawDescriptor::new().with_creation_time(1).flags.contains(Flags::CREATETIME));
    assert!(RawDescriptor::new().with_last_access_time(1).flags.contains(Flags::ACCESSTIME));
    assert!(RawDescriptor::new().with_clsid([0; 16]).flags.contains(Flags::CLSID));
    assert_eq!(RawDescriptor::new().flags, Flags::NONE);
}

#[test]
fn an_empty_builder_emits_just_c_items() {
    let bytes = Builder::new().finish();
    assert_eq!(bytes, [0, 0, 0, 0]);
    assert!(FileGroupDescriptor::parse(&bytes).unwrap().is_empty());
}

#[test]
fn a_name_that_leaves_no_room_for_the_nul_is_refused() {
    let mut b = Builder::new();
    let ok = "x".repeat(MAX_WRITABLE_NAME_UNITS);
    b.push(RawDescriptor::new(), &ok).unwrap();

    let too_long = "x".repeat(FILE_NAME_UNITS);
    let err = Builder::new().push(RawDescriptor::new(), &too_long).unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooLarge, "260 units leaves nowhere for the terminator");
}

#[test]
fn name_length_is_counted_in_utf16_units_not_chars() {
    // An astral char costs two units, so 130 of them fill the field.
    let emoji = "\u{1F4C4}".repeat(130);
    assert_eq!(emoji.chars().count(), 130);
    let err = Builder::new().push(RawDescriptor::new(), &emoji).unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooLarge);

    let just_fits = "\u{1F4C4}".repeat(129);
    Builder::new().push(RawDescriptor::new(), &just_fits).unwrap();
}

#[test]
fn an_embedded_nul_in_a_name_is_refused() {
    let err = Builder::new().push(RawDescriptor::new(), "a\0b.txt").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed, "it would truncate on the way back in");

    let name: Vec<u8> = "a\0b".encode_utf16().flat_map(u16::to_le_bytes).collect();
    let err = Builder::new().push_utf16_name(RawDescriptor::new(), &name).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn an_odd_length_utf16_name_is_a_bad_length() {
    let err = Builder::new().push_utf16_name(RawDescriptor::new(), b"abc").unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLength);
}

#[test]
fn every_prefix_of_every_fixture_either_parses_or_errors() {
    for (name, _) in sidecar_expectations() {
        let bytes = fixture(&name);
        for len in 0..=bytes.len() {
            if let Ok(group) = FileGroupDescriptor::parse(&bytes[..len]) {
                // Walking is part of the surface a fuzzer would reach.
                for d in group.iter() {
                    let _ = d.file_name_lossy();
                    let _ = d.file_size();
                }
            }
        }
    }
}

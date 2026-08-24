//! `CFSTR_FILEDESCRIPTORW` against the synthetic corpus.
//!
//! Two things are being pinned down here. First, that `cItems` is checked
//! against the buffer before anything iterates on it — that field is a `u32`
//! from another process. Second, that a clear flag reads as `None` and never as
//! the plausible-looking zero (or, in the corpus, `0xDEADBEEF`) sitting in the
//! field.

use rclip_core::ErrorKind;
use rclip_file_desc::{
    file_attribute, Builder, BuilderA, FileDescriptor, FileDescriptorA, FileGroupDescriptor,
    FileGroupDescriptorA, Flags, PointL, RawDescriptor, SizeL, DESCRIPTOR_A_LEN, DESCRIPTOR_LEN,
    FILE_NAME_BYTES, FILE_NAME_UNITS, GROUP_HEADER_LEN, MAX_WRITABLE_NAME_BYTES,
    MAX_WRITABLE_NAME_UNITS,
};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/");
    std::fs::read(format!("{p}rclip-file-desc/{name}")).expect("fixture")
}

fn sidecar_expectations() -> Vec<(String, String, String)> {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-file-desc"
    );
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
        let format = json
            .split("\"format\"")
            .nth(1)
            .and_then(|s| s.split('"').nth(1))
            .expect("sidecar has a \"format\" field")
            .to_owned();
        let stem = path.file_stem().unwrap().to_str().unwrap().to_owned();
        out.push((format!("{stem}.bin"), expect, format));
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
    assert_eq!(
        MAX_WRITABLE_NAME_UNITS, 259,
        "one unit is reserved for the NUL"
    );
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
    let mut saw_ansi = false;
    for (name, expect, format) in cases {
        let bytes = fixture(&name);
        // Nothing in the payload says which layout it is -- a descriptor is 592
        // bytes wide and 332 ANSI, and neither carries a marker. The clipboard
        // format name is the only thing that decides, which is why this crate
        // refuses to sniff. So the sweep routes on the sidecar's format, the
        // same way the corpus gate does.
        let is_ansi = format.eq_ignore_ascii_case("FILEGROUPDESCRIPTORA")
            || format.eq_ignore_ascii_case("CFSTR_FILEDESCRIPTOR");
        saw_ansi |= is_ansi;
        let parsed = if is_ansi {
            FileGroupDescriptorA::parse(&bytes).map(|_| ())
        } else {
            FileGroupDescriptor::parse(&bytes).map(|_| ())
        };
        match expect.as_str() {
            "ok" => {
                parsed.unwrap_or_else(|e| panic!("{name} is declared ok but failed: {e}"));
            }
            "error" => {
                assert!(
                    parsed.is_err(),
                    "{name} is declared error but parsed cleanly"
                );
            }
            other => panic!("{name}: unknown expect value {other:?}"),
        }
    }
    assert!(
        saw_ansi,
        "the ANSI layout has no corpus fixture, so this sweep only ever exercises the wide \
         reading and the format-based routing above is untested"
    );
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
    assert_eq!(
        second.clsid(),
        Some([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
    );
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
    assert_eq!(
        second.raw().file_size,
        0xDEAD_BEEF,
        "but the bytes are still there"
    );
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
        assert_eq!(
            err.kind,
            ErrorKind::UnexpectedEof,
            "{len}-byte payload must not panic"
        );
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
    assert_eq!(
        d.file_name_chars().map(Result::unwrap).collect::<String>(),
        "empty.txt"
    );
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
        FileDescriptor::parse(&[0u8; DESCRIPTOR_LEN - 1])
            .unwrap_err()
            .kind,
        ErrorKind::BadLength
    );
    assert_eq!(
        FileDescriptor::parse(&[0u8; DESCRIPTOR_LEN + 1])
            .unwrap_err()
            .kind,
        ErrorKind::BadLength
    );
}

#[test]
fn get_is_bounded_by_the_declared_count() {
    let bytes = fixture("two-descriptors.bin");
    let group = FileGroupDescriptor::parse(&bytes).unwrap();
    assert!(group.get(1).is_some());
    assert!(group.get(2).is_none());
    assert!(
        group.get(usize::MAX).is_none(),
        "must not overflow when scaling the index"
    );
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
    for name in [
        "two-descriptors.bin",
        "zero-length-file.bin",
        "empty-group.bin",
    ] {
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
    assert!(RawDescriptor::new()
        .with_icon(SizeL::default(), PointL::default())
        .flags
        .contains(Flags::SIZEPOINT));
    assert!(RawDescriptor::new()
        .with_creation_time(1)
        .flags
        .contains(Flags::CREATETIME));
    assert!(RawDescriptor::new()
        .with_last_access_time(1)
        .flags
        .contains(Flags::ACCESSTIME));
    assert!(RawDescriptor::new()
        .with_clsid([0; 16])
        .flags
        .contains(Flags::CLSID));
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
    let err = Builder::new()
        .push(RawDescriptor::new(), &too_long)
        .unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::TooLarge,
        "260 units leaves nowhere for the terminator"
    );
}

#[test]
fn name_length_is_counted_in_utf16_units_not_chars() {
    // An astral char costs two units, so 130 of them fill the field.
    let emoji = "\u{1F4C4}".repeat(130);
    assert_eq!(emoji.chars().count(), 130);
    let err = Builder::new()
        .push(RawDescriptor::new(), &emoji)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooLarge);

    let just_fits = "\u{1F4C4}".repeat(129);
    Builder::new()
        .push(RawDescriptor::new(), &just_fits)
        .unwrap();
}

#[test]
fn an_embedded_nul_in_a_name_is_refused() {
    let err = Builder::new()
        .push(RawDescriptor::new(), "a\0b.txt")
        .unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::Malformed,
        "it would truncate on the way back in"
    );

    let name: Vec<u8> = "a\0b".encode_utf16().flat_map(u16::to_le_bytes).collect();
    let err = Builder::new()
        .push_utf16_name(RawDescriptor::new(), &name)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn an_odd_length_utf16_name_is_a_bad_length() {
    let err = Builder::new()
        .push_utf16_name(RawDescriptor::new(), b"abc")
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLength);
}

#[test]
fn every_prefix_of_every_fixture_either_parses_or_errors() {
    for (name, _, _) in sidecar_expectations() {
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

// ------------------------------------------------- FILEGROUPDESCRIPTORA -----
//
// The ANSI twin. Same struct, one field narrower: `CHAR cFileName[260]` instead
// of `WCHAR`, so 332 bytes per descriptor rather than 592.

#[test]
fn the_ansi_struct_sizes_are_what_we_encode() {
    // 4 + 16 + 8 + 8 + 4 + 8 + 8 + 8 + 4 + 4 + 260. Same 72-byte prefix, a name
    // field half the width.
    assert_eq!(DESCRIPTOR_A_LEN, 332);
    assert_eq!(GROUP_HEADER_LEN + DESCRIPTOR_A_LEN, 336);
    assert_eq!(FILE_NAME_BYTES, 260, "cFileName is CHAR[MAX_PATH]");
    assert_eq!(
        MAX_WRITABLE_NAME_BYTES, 259,
        "one byte is reserved for the NUL"
    );
    assert_eq!(
        DESCRIPTOR_LEN - DESCRIPTOR_A_LEN,
        260,
        "the two differ by exactly the extra byte per name character"
    );
}

fn ansi_group() -> Vec<u8> {
    let mut b = BuilderA::new();
    b.push_ansi_name(
        RawDescriptor::new()
            .with_file_size(4096)
            .with_attributes(file_attribute::NORMAL)
            .with_progress_ui(),
        b"report.pdf",
    )
    .unwrap();
    b.push_ansi_name(RawDescriptor::new(), b"sub\\notes.txt")
        .unwrap();
    b.finish()
}

#[test]
fn an_ansi_group_round_trips_through_the_builder() {
    let bytes = ansi_group();
    assert_eq!(bytes.len(), GROUP_HEADER_LEN + 2 * DESCRIPTOR_A_LEN);

    let group = FileGroupDescriptorA::parse(&bytes).unwrap();
    assert_eq!(group.len(), 2);
    assert!(!group.is_empty());

    let first = group.get(0).unwrap();
    assert_eq!(first.file_name_ansi(), b"report.pdf");
    assert_eq!(first.file_size(), Some(4096));
    assert_eq!(first.file_attributes(), Some(file_attribute::NORMAL));
    assert!(first.wants_progress_ui());
    assert!(!first.is_directory());

    let second = group.get(1).unwrap();
    assert_eq!(
        second.file_name_ansi(),
        b"sub\\notes.txt",
        "a relative path is what real producers put in cFileName for a tree"
    );
    // A clear flag reads as None, never as the plausible-looking zero.
    assert_eq!(second.file_size(), None);
    assert_eq!(second.file_attributes(), None);
    assert_eq!(second.last_write_time(), None);

    // Iteration and indexing must agree.
    let walked: Vec<FileDescriptorA<'_>> = group.iter().collect();
    assert_eq!(walked.len(), 2);
    assert_eq!(walked[0], first);
    assert_eq!(group.iter().len(), 2, "ExactSizeIterator");

    // And re-emitting a parsed descriptor is byte-identical.
    let mut again = BuilderA::new();
    for d in &group {
        again.push_descriptor(&d).unwrap();
    }
    assert_eq!(again.finish(), bytes);
}

#[test]
fn the_72_byte_prefix_is_identical_in_both_spellings() {
    // This is the whole reason both parsers share one reader: everything before
    // cFileName is byte-for-byte the same struct.
    let raw = RawDescriptor::new()
        .with_file_size(0x1_0000_0007)
        .with_attributes(file_attribute::ARCHIVE)
        .with_last_write_time(0x01D9_ABCD_1234_5678)
        .with_clsid([7u8; 16])
        .with_icon(SizeL { cx: 32, cy: 48 }, PointL { x: -1, y: 2 });

    let mut w = Builder::new();
    w.push(raw, "x").unwrap();
    let wide = w.finish();
    let mut a = BuilderA::new();
    a.push_ansi_name(raw, b"x").unwrap();
    let ansi = a.finish();

    assert_eq!(
        wide[GROUP_HEADER_LEN..GROUP_HEADER_LEN + 72],
        ansi[GROUP_HEADER_LEN..GROUP_HEADER_LEN + 72]
    );

    let wd = FileGroupDescriptor::parse(&wide).unwrap().get(0).unwrap();
    let ad = FileGroupDescriptorA::parse(&ansi).unwrap().get(0).unwrap();
    assert_eq!(wd.raw(), ad.raw());
    assert_eq!(wd.file_size(), ad.file_size());
    assert_eq!(wd.last_write_time(), ad.last_write_time());
    assert_eq!(wd.clsid(), ad.clsid());
    assert_eq!(wd.icon_size(), ad.icon_size());
    assert_eq!(wd.icon_position(), ad.icon_position());
}

#[test]
fn an_ansi_c_items_is_checked_against_the_buffer_before_anything_iterates() {
    let mut bytes = ansi_group();
    bytes[..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    assert_eq!(
        FileGroupDescriptorA::parse(&bytes).unwrap_err().kind,
        ErrorKind::TooLarge,
        "0xFFFFFFFF x 332 is a 1.3 TiB read otherwise"
    );

    // One descriptor promised, most of it missing.
    let short = &ansi_group()[..GROUP_HEADER_LEN + 100];
    assert_eq!(
        FileGroupDescriptorA::parse(short).unwrap_err().kind,
        ErrorKind::TooLarge
    );

    // Not even the cItems word.
    assert_eq!(
        FileGroupDescriptorA::parse(&[0u8, 0, 0]).unwrap_err().kind,
        ErrorKind::UnexpectedEof
    );

    // An empty group is legal, if pointless.
    let zero = 0u32.to_le_bytes();
    let empty = FileGroupDescriptorA::parse(&zero).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.iter().count(), 0);
}

#[test]
fn the_ansi_name_field_is_truncated_at_its_first_nul() {
    // Everything after the terminator is padding and must not leak out, even
    // when a producer left something in it.
    let mut bytes = ansi_group();
    let name_at = GROUP_HEADER_LEN + 72;
    bytes[name_at + "report.pdf".len() + 1..name_at + 40].fill(b'X');

    let d = FileGroupDescriptorA::parse(&bytes).unwrap().get(0).unwrap();
    assert_eq!(d.file_name_ansi(), b"report.pdf");
}

#[test]
fn a_name_that_fills_the_field_leaves_no_room_for_the_terminator() {
    let mut b = BuilderA::new();
    assert_eq!(
        b.push_ansi_name(RawDescriptor::new(), &[b'a'; FILE_NAME_BYTES])
            .unwrap_err()
            .kind,
        ErrorKind::TooLarge
    );
    // One byte shorter fits exactly.
    b.push_ansi_name(RawDescriptor::new(), &[b'a'; MAX_WRITABLE_NAME_BYTES])
        .unwrap();

    // An embedded NUL would truncate the name on the way back in, which makes
    // it a different file rather than a display problem.
    assert_eq!(
        b.push_ansi_name(RawDescriptor::new(), b"a\0b")
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
}

#[test]
fn fd_unicode_on_an_ansi_descriptor_is_reported_not_rejected() {
    let mut b = BuilderA::new();
    b.push_ansi_name(RawDescriptor::new().with_unicode(), b"confused.txt")
        .unwrap();
    let bytes = b.finish();

    let d = FileGroupDescriptorA::parse(&bytes).unwrap().get(0).unwrap();
    assert!(
        d.claims_unicode(),
        "a producer that sets FD_UNICODE on a CHAR[260] name is confused, and \
         knowing that is more useful than a refusal to parse"
    );
    assert_eq!(d.file_name_ansi(), b"confused.txt");
}

#[test]
fn the_two_spellings_are_told_apart_by_the_format_name_and_not_by_sniffing() {
    // A wide group is 596 bytes for one descriptor, which is more than the 336
    // an ANSI group needs — so it "parses" as ANSI and yields nonsense. That is
    // the documented contract: which reader to use comes from the clipboard
    // format name ("FileGroupDescriptorW" vs "FileGroupDescriptor"), because a
    // length test is wrong exactly when the trailing slack makes both fit.
    let mut w = Builder::new();
    w.push(RawDescriptor::new().with_file_size(7), "note.txt")
        .unwrap();
    let wide = w.finish();

    let as_ansi = FileGroupDescriptorA::parse(&wide).unwrap();
    assert_eq!(as_ansi.len(), 1);
    assert_ne!(
        as_ansi.get(0).unwrap().file_name_ansi(),
        b"note.txt",
        "UTF-16LE read as bytes is not the same name; nothing here pretends otherwise"
    );
}

#[test]
fn one_ansi_descriptor_must_be_exactly_332_bytes() {
    let bytes = ansi_group();
    let one = &bytes[GROUP_HEADER_LEN..GROUP_HEADER_LEN + DESCRIPTOR_A_LEN];
    assert!(FileDescriptorA::parse(one).is_ok());
    assert_eq!(
        FileDescriptorA::parse(&one[..DESCRIPTOR_A_LEN - 1])
            .unwrap_err()
            .kind,
        ErrorKind::BadLength
    );
    assert_eq!(
        FileDescriptorA::parse(&bytes[GROUP_HEADER_LEN..])
            .unwrap_err()
            .kind,
        ErrorKind::BadLength,
        "too long is as wrong as too short: every field is fixed-width"
    );
}

#[test]
fn no_ansi_prefix_panics() {
    // The fuzzer's job, done by hand: every prefix of a good payload must
    // answer rather than panic.
    let bytes = ansi_group();
    for len in 0..=bytes.len() {
        if let Ok(group) = FileGroupDescriptorA::parse(&bytes[..len]) {
            for d in group.iter() {
                let _ = d.file_name_ansi();
                let _ = d.file_size();
            }
        }
    }
}

// The code page is not in the payload, so everything below names it explicitly.
#[cfg(feature = "codepage")]
mod ansi_codepage {
    use super::*;
    use rclip_codepage::Encoding;

    fn cp1252() -> Encoding {
        Encoding::from_windows_codepage(1252).expect("windows-1252 is in the table")
    }

    #[test]
    fn a_name_encodes_and_decodes_through_a_named_code_page() {
        let mut b = BuilderA::new();
        // U+201C, a left double quote: 0x93 in windows-1252 and a C1 control in
        // Latin-1, which is the pair this whole feature exists to keep apart.
        b.push_str_with(RawDescriptor::new(), "\u{201C}quoted\u{201D}.txt", cp1252())
            .unwrap();
        let bytes = b.finish();

        let d = FileGroupDescriptorA::parse(&bytes).unwrap().get(0).unwrap();
        assert_eq!(d.file_name_ansi()[0], 0x93);
        assert_eq!(
            d.file_name_with(cp1252()).unwrap(),
            "\u{201C}quoted\u{201D}.txt"
        );
    }

    #[test]
    fn a_character_the_code_page_cannot_represent_is_refused_not_substituted() {
        let mut b = BuilderA::new();
        assert_eq!(
            b.push_str_with(RawDescriptor::new(), "s\u{142}owo.txt", cp1252())
                .unwrap_err()
                .kind,
            ErrorKind::Unsupported,
            "substituting here would produce a different file name, not a glyph"
        );
        assert!(b.is_empty(), "a refused push must leave nothing behind");
    }

    #[test]
    fn an_undefined_byte_is_an_error_strictly_and_u_fffd_lossily() {
        // 0x81 is one of the five bytes windows-1252 leaves unassigned.
        let mut b = BuilderA::new();
        b.push_ansi_name(RawDescriptor::new(), b"a\x81b.txt")
            .unwrap();
        let bytes = b.finish();

        let d = FileGroupDescriptorA::parse(&bytes).unwrap().get(0).unwrap();
        assert_eq!(
            d.file_name_with(cp1252()).unwrap_err().kind,
            ErrorKind::Malformed
        );
        assert_eq!(d.file_name_lossy_with(cp1252()), "a\u{FFFD}b.txt");
    }

    #[test]
    fn the_length_limit_is_bytes_and_is_measured_after_encoding() {
        let mut b = BuilderA::new();
        // 259 characters that each cost one byte in windows-1252: exactly fits.
        let name = "a".repeat(MAX_WRITABLE_NAME_BYTES);
        b.push_str_with(RawDescriptor::new(), &name, cp1252())
            .unwrap();
        assert_eq!(
            b.push_str_with(RawDescriptor::new(), &format!("{name}a"), cp1252())
                .unwrap_err()
                .kind,
            ErrorKind::TooLarge
        );
    }
}

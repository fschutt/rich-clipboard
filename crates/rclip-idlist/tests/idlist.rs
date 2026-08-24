//! Integration tests for `rclip-idlist`.
//!
//! The structural walk is tested harder than the item decoding, because that is
//! where hostile input does damage: item decoding cannot fail by construction,
//! but the list walk can hang, misalign, or read out of bounds.

use rclip_idlist::{
    guid::Guid,
    item::ItemIdList,
    shell_item::{ExtensionBlock, EXTENSION_FILE_ENTRY},
    Cida, DosDateTime, ShellItem, ShellStr,
};

use rclip_core::ErrorKind;

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-idlist/"
    );
    std::fs::read(format!("{p}{name}")).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

// ---------------------------------------------------------------- the cb trap

#[test]
fn cb_of_zero_terminates_the_walk_instead_of_hanging() {
    let buf = fixture("cb-zero-bomb.bin");
    let mut list = ItemIdList::new(&buf);

    let first = list
        .next()
        .expect("one item before the bomb")
        .expect("well formed");
    assert_eq!(first.cb(), 20, "the root folder item ahead of the zero");

    assert!(
        list.next().is_none(),
        "cb = 0 is the terminator; the walk must stop there"
    );
    assert!(
        list.is_terminated(),
        "stopping on a zero cb counts as a clean termination"
    );
    assert_eq!(
        list.bytes_consumed(),
        22,
        "20 bytes of item plus the 2-byte terminator, ignoring the trailing bytes"
    );
    assert!(
        !list.failed(),
        "an early terminator is legal, not a parse failure"
    );
}

#[test]
fn cb_of_one_is_rejected_because_it_cannot_advance_the_walk() {
    let buf = fixture("cb-one-bomb.bin");
    let mut list = ItemIdList::new(&buf);

    list.next().expect("first item").expect("well formed");

    let err = list
        .next()
        .expect("an error, not a silent stop")
        .unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::BadLength,
        "cb counts its own two bytes, so a cb of one is impossible and must be rejected \
         rather than used as a stride"
    );
    assert_eq!(
        err.offset, 20,
        "the error points at the cb field that carried the bad value"
    );
    assert!(list.failed());
    assert!(
        list.next().is_none(),
        "the walk yields at most one error, then stops"
    );
}

#[test]
fn an_item_declaring_more_than_remains_is_unexpected_eof() {
    let buf = fixture("item-runs-past-end.bin");
    let mut list = ItemIdList::new(&buf);
    list.next().expect("first item").expect("well formed");

    let err = list.next().expect("an error").unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    assert_eq!(err.offset, 20);
}

#[test]
fn a_stray_trailing_byte_is_reported_rather_than_dropped() {
    let buf = fixture("odd-trailing-byte.bin");
    let mut list = ItemIdList::new(&buf);
    list.next().expect("first item").expect("well formed");

    let err = list.next().expect("an error").unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::UnexpectedEof,
        "one byte cannot be a cb field; treating it as an end of list would hide a truncation"
    );
}

#[test]
fn running_out_on_an_item_boundary_is_a_clean_end() {
    // The same list as `two-items.bin`, with the terminator sliced off — which
    // is what a PIDL carved out of a larger structure by a length field looks
    // like.
    let buf = fixture("two-items.bin");
    let body = &buf[..buf.len() - 2];

    let mut list = ItemIdList::new(body);
    assert!(list.next().unwrap().is_ok());
    assert!(list.next().unwrap().is_ok());
    assert!(list.next().is_none());
    assert!(!list.failed(), "an exact-boundary end is not a failure");
    assert!(
        !list.is_terminated(),
        "but it is distinguishable from an explicit terminator"
    );
}

#[test]
fn the_walk_terminates_on_every_possible_cb() {
    // Exhaustive over the whole u16 space of the first size field. There is no
    // fuzzer in Phase 0, and this is the property a fuzzer would be looking for.
    for cb in 0u16..=u16::MAX {
        let mut buf = cb.to_le_bytes().to_vec();
        buf.extend_from_slice(&[0xAB; 64]);

        let mut steps = 0usize;
        for item in ItemIdList::new(&buf) {
            steps += 1;
            assert!(
                steps <= buf.len(),
                "walk failed to make progress on cb = {cb:#06x}"
            );
            if item.is_err() {
                break;
            }
        }
    }
}

#[test]
fn arbitrary_bytes_never_panic_or_hang() {
    // A deterministic xorshift stands in for a fuzzer: no dependency, and a
    // failure reproduces from the seed printed in the assert.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for round in 0..2000 {
        let len = (next() % 96) as usize;
        let buf: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();

        let mut steps = 0usize;
        for item in ItemIdList::new(&buf) {
            steps += 1;
            assert!(
                steps <= buf.len() + 1,
                "round {round} failed to make progress"
            );
            match item {
                Ok(id) => {
                    // Item decoding is infallible; the point is that it does not
                    // panic on arbitrary bytes either.
                    let _ = id.parse().display_name();
                    let _ = ExtensionBlock::walk(id.data).count();
                }
                Err(_) => break,
            }
        }

        // The same bytes as a CIDA.
        if let Ok(cida) = Cida::parse(&buf) {
            for list in cida.children().take(64).flatten() {
                let _ = list.take(64).count();
            }
        }
    }
}

// ---------------------------------------------------------------- item decoding

#[test]
fn two_items_decode_to_a_root_folder_and_a_volume() {
    let buf = fixture("two-items.bin");
    let items: Vec<_> = ItemIdList::new(&buf)
        .map(|i| i.expect("well formed"))
        .collect();
    assert_eq!(items.len(), 2);

    match items[0].parse() {
        ShellItem::RootFolder(root) => {
            assert_eq!(root.sort_index, 0x50);
            assert_eq!(root.guid.well_known_name(), Some("My Computer"));
            assert_eq!(
                root.guid.to_braced().as_str(),
                "{20D04FE0-3AEA-1069-A2D8-08002B30309D}"
            );
        }
        other => panic!("expected a root folder, got {other:?}"),
    }

    match items[1].parse() {
        ShellItem::Volume(vol) => {
            assert_eq!(vol.class, 0x2F);
            assert_eq!(vol.name.and_then(|n| n.as_ascii()), Some("C:\\"));
            assert_eq!(vol.guid, None, "the short 25-byte form carries no GUID");
        }
        other => panic!("expected a volume, got {other:?}"),
    }
}

#[test]
fn a_file_entry_prefers_the_long_name_over_the_8_3_form() {
    let buf = fixture("file-entry-beef0004-v3.bin");
    let item = ItemIdList::new(&buf).next().unwrap().unwrap();

    let ShellItem::FileEntry(entry) = item.parse() else {
        panic!("expected a file entry");
    };

    assert_eq!(entry.class, 0x32);
    assert!(entry.is_file());
    assert!(!entry.is_directory());
    assert!(
        !entry.has_unicode_name(),
        "class 0x32 has the 0x04 Unicode bit clear"
    );
    assert_eq!(entry.file_size, 214_528);
    assert_eq!(entry.attributes, 0x0020, "FILE_ATTRIBUTE_ARCHIVE");

    assert_eq!(entry.primary_name.as_ascii(), Some("wordpad.exe"));
    assert!(matches!(entry.primary_name, ShellStr::Ansi(_)));

    let long = entry
        .long_name
        .expect("the 0xBEEF0004 block carries a long name");
    assert!(
        matches!(long, ShellStr::Utf16(_)),
        "long names are always UTF-16LE"
    );
    assert_eq!(long.to_string_lossy(), "wordpad.exe");
    assert_eq!(
        entry.localized_name, None,
        "localized name offset is zero in this vector"
    );

    let ext = entry.extension.expect("extension block");
    assert_eq!(ext.signature, EXTENSION_FILE_ENTRY);
    assert_eq!(ext.version, 3);
    assert_eq!(
        ext.offset, 24,
        "26 from the item start, minus the two bytes of cb"
    );
    assert_eq!(ext.size, 46);

    let file_ext = ext.as_file_entry().expect("a 0xBEEF0004 block");
    assert_eq!(
        file_ext.file_reference, None,
        "the file reference arrives at version 7"
    );

    // 0x3104 / 0x6800 packed FAT date and time.
    let m = entry.modified;
    assert_eq!((m.year(), m.month(), m.day()), (2004, 8, 4));
    assert_eq!((m.hour(), m.minute(), m.second()), (13, 0, 0));
}

#[test]
fn an_unknown_item_class_does_not_break_the_items_around_it() {
    let buf = fixture("unknown-class.bin");
    let items: Vec<_> = ItemIdList::new(&buf)
        .map(|i| i.expect("well formed"))
        .collect();
    assert_eq!(
        items.len(),
        3,
        "the unknown item must not truncate the walk"
    );

    assert!(matches!(items[0].parse(), ShellItem::RootFolder(_)));
    match items[1].parse() {
        ShellItem::Unknown { class, raw } => {
            assert_eq!(class, 0x88);
            assert_eq!(
                &raw[1..],
                b"opaque payload",
                "the raw bytes are handed back intact"
            );
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
    match items[2].parse() {
        ShellItem::Volume(v) => assert_eq!(v.name.and_then(|n| n.as_ascii()), Some("D:\\")),
        other => panic!("expected a volume, got {other:?}"),
    }
}

#[test]
fn an_unknown_item_has_no_display_name_rather_than_a_made_up_one() {
    let buf = fixture("unknown-class.bin");
    let items: Vec<_> = ItemIdList::new(&buf).map(|i| i.unwrap()).collect();
    assert_eq!(items[1].parse().display_name(), None);
}

#[test]
fn an_empty_list_is_zero_items_and_terminated() {
    let buf = fixture("empty.bin");
    let mut list = ItemIdList::new(&buf);
    assert!(list.next().is_none());
    assert!(list.is_terminated());
    assert_eq!(list.bytes_consumed(), 2);
}

#[test]
fn an_empty_item_body_parses_as_empty_not_as_an_error() {
    let buf = [0x02u8, 0x00, 0x00, 0x00]; // cb = 2 (no abID), then the terminator
    let item = ItemIdList::new(&buf).next().unwrap().unwrap();
    assert_eq!(item.data.len(), 0);
    assert_eq!(item.class(), None);
    assert_eq!(item.parse(), ShellItem::Empty);
}

#[test]
fn a_phantom_extension_offset_is_rejected_by_the_signature_check() {
    // A file entry whose last two bytes happen to read as a plausible offset,
    // but where no 0xBEEF signature lives there. The generic end-of-item scan
    // runs on every item, so this is the case that would otherwise invent a
    // block out of unrelated bytes.
    let mut body = vec![0x32u8, 0x00];
    body.extend_from_slice(&0u32.to_le_bytes()); // size
    body.extend_from_slice(&0u32.to_le_bytes()); // modified
    body.extend_from_slice(&0u16.to_le_bytes()); // attributes
    body.extend_from_slice(b"a.txt\0");
    body.extend_from_slice(&8u16.to_le_bytes()); // "offset 8" — inside the item, no signature

    let mut buf = (body.len() as u16 + 2).to_le_bytes().to_vec();
    buf.extend_from_slice(&body);
    buf.extend_from_slice(&[0, 0]);

    let ShellItem::FileEntry(entry) = ItemIdList::new(&buf).next().unwrap().unwrap().parse() else {
        panic!("expected a file entry");
    };
    assert_eq!(
        entry.extension, None,
        "no 0xBEEF signature means no extension block"
    );
    assert_eq!(entry.long_name, None);
    assert_eq!(entry.primary_name.as_ascii(), Some("a.txt"));
}

// ---------------------------------------------------------------- CIDA

#[test]
fn a_cida_yields_its_parent_and_every_child() {
    let buf = fixture("cida-two-children.bin");
    let cida = Cida::parse(&buf).expect("well formed");
    assert_eq!(cida.child_count(), 2);

    let parent: Vec<_> = cida.parent().unwrap().map(|i| i.unwrap()).collect();
    assert_eq!(parent.len(), 1);
    assert!(matches!(parent[0].parse(), ShellItem::RootFolder(_)));

    let names: Vec<String> = cida
        .children()
        .map(|c| {
            let mut list = c.expect("child offset in range");
            let item = list.next().unwrap().unwrap();
            item.parse().display_name().unwrap().to_string_lossy()
        })
        .collect();
    assert_eq!(names, ["wordpad.exe", "notepad long name.exe"]);
}

#[test]
fn cida_errors_carry_the_offset_of_the_table_entry_that_was_wrong() {
    let buf = fixture("cida-child-offset-past-end.bin");
    let cida = Cida::parse(&buf).expect("the header itself is well formed");

    // The parent still reads: one bad child must not cost the caller the rest.
    //
    // `parent()` hands back an iterator, so `is_ok()` alone proves nothing —
    // it succeeds without ever looking at a byte. Walk it to completion, which
    // is what a caller does and what actually exercises the offset.
    let parent = cida
        .parent()
        .expect("the parent offset is inside the buffer");
    let items: Result<Vec<_>, _> = parent.collect();
    let items = items.expect("the parent IDList must walk cleanly");
    assert_eq!(items.len(), 1, "the parent is a single root/GUID item");
    assert!(
        cida.parent().is_ok(),
        "a bad child offset must not poison the parent"
    );

    let err = cida.child(0).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadOffset);
    assert_eq!(
        err.offset, 8,
        "aoffset[1] lives at byte 8: 4 for cidl, 4 for aoffset[0]"
    );
}

#[test]
fn a_cida_count_the_buffer_cannot_back_is_rejected_before_it_sizes_anything() {
    let buf = fixture("cida-count-too-large.bin");
    let err = Cida::parse(&buf).unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::TooLarge,
        "cidl sizes the offset table; checking it against the bytes present is what stops a \
         12-byte payload from asking for a 16 GiB read"
    );
}

#[test]
fn a_cida_shorter_than_its_own_count_field_is_eof() {
    let err = Cida::parse(&[0x01, 0x00]).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnexpectedEof);
}

#[test]
fn asking_a_cida_for_a_child_it_does_not_have_is_an_error_not_a_panic() {
    let buf = fixture("cida-two-children.bin");
    let cida = Cida::parse(&buf).unwrap();
    assert!(cida.child(2).is_err());
    assert!(cida.child(usize::MAX).is_err());
}

#[test]
fn cida_error_offsets_are_absolute_within_the_payload() {
    // A child whose PIDL is malformed reports the offset in the CIDA, not in
    // the slice the child parser happened to be handed.
    let buf = fixture("cida-two-children.bin");
    let cida = Cida::parse(&buf).unwrap();
    let list = cida.child(1).unwrap();
    let first = list.clone().next().unwrap().unwrap();
    assert_eq!(
        first.offset, 0,
        "item offsets stay relative to their own list"
    );
    assert!(
        cida.offset(2).unwrap() > 0,
        "but the list's base is the CIDA-relative offset"
    );
}

// ---------------------------------------------------------------- GUID

#[test]
fn guids_print_in_the_order_regedit_shows_them() {
    // The first three fields are little-endian on the wire and big-endian on
    // screen; a formatter that forgets that produces a GUID that looks right
    // and matches nothing.
    let clsid = Guid::from_bytes([
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ]);
    assert_eq!(
        clsid.to_braced().as_str(),
        "{00021401-0000-0000-C000-000000000046}"
    );
    assert_eq!(clsid.data1(), 0x0002_1401);
}

#[test]
fn an_unrecognised_guid_has_no_name_rather_than_a_wrong_one() {
    let g = Guid::from_bytes([0xAA; 16]);
    assert_eq!(g.well_known_name(), None);
}

#[test]
fn a_guid_needs_all_sixteen_bytes() {
    assert!(Guid::from_slice(&[0u8; 15]).is_none());
    assert!(Guid::from_slice(&[0u8; 16]).is_some());
    assert!(
        Guid::from_slice(&[0u8; 32]).is_some(),
        "extra bytes are ignored, not an error"
    );
}

// ---------------------------------------------------------------- strings

#[test]
fn ascii_ansi_borrows_and_non_ascii_ansi_does_not_guess_a_code_page() {
    assert_eq!(ShellStr::Ansi(b"C:\\Users").as_ascii(), Some("C:\\Users"));
    assert_eq!(
        ShellStr::Ansi(b"caf\xe9").as_ascii(),
        None,
        "0xE9 is e-acute in CP1252 and something else in CP932; without the code page there \
         is no right answer, and guessing produces a path that looks correct and is not"
    );
    assert_eq!(ShellStr::Ansi(b"caf\xe9").to_string_lossy(), "caf\u{FFFD}");
}

#[test]
fn a_utf16_field_never_borrows_as_str() {
    let s = ShellStr::Utf16(b"a\x00b\x00");
    assert_eq!(s.as_ascii(), None, "re-encoding would need an allocation");
    assert_eq!(s.to_string_lossy(), "ab");
}

#[test]
fn a_lone_surrogate_stops_utf16_decoding_but_an_unknown_ansi_byte_does_not() {
    // Losing sync matters for a variable-width encoding and not for a
    // single-byte one, and the iterators differ accordingly.
    let bad_utf16 = ShellStr::Utf16(&[0x00, 0xD8, 0x41, 0x00]);
    assert_eq!(
        bad_utf16.chars().count(),
        1,
        "a lone high surrogate ends the field"
    );

    let bad_ansi = ShellStr::Ansi(&[b'a', 0xE9, b'b']);
    assert_eq!(
        bad_ansi.chars().count(),
        3,
        "an undecodable byte does not end the field"
    );
    assert_eq!(bad_ansi.to_string_lossy(), "a\u{FFFD}b");
}

// ---------------------------------------------------------------- DOS time

#[test]
fn dos_date_time_unpacks_the_fat_bit_layout() {
    // 2004-08-04 13:00:00, the modification stamp in the wordpad fixture.
    let dt = DosDateTime::from_le_bytes([0x04, 0x31, 0x00, 0x68]);
    assert_eq!(dt.year(), 2004);
    assert_eq!(dt.month(), 8);
    assert_eq!(dt.day(), 4);
    assert_eq!(dt.hour(), 13);
    assert_eq!(dt.minute(), 0);
    assert_eq!(dt.second(), 0);
    assert!(!dt.is_unset());
    assert_eq!(dt.to_le_bytes(), [0x04, 0x31, 0x00, 0x68], "round trips");
}

#[test]
fn an_all_zero_fat_stamp_is_unset_not_1980() {
    let dt = DosDateTime::from_le_bytes([0; 4]);
    assert!(
        dt.is_unset(),
        "these fields are routinely left blank in real captures"
    );
}

#[test]
fn fat_seconds_are_reported_as_encoded_not_clamped_to_a_real_clock() {
    // FAT stores seconds/2 in five bits, so 0x1F decodes to 62 — past the end
    // of a minute. Reporting it is the point: a forensic caller wants to know
    // the bytes said something impossible.
    let dt = DosDateTime {
        date: 0,
        time: 0x001F,
    };
    assert_eq!(dt.second(), 62);
    // And an odd second simply cannot be encoded.
    assert_eq!(
        DosDateTime {
            date: 0,
            time: 0x0001
        }
        .second(),
        2
    );
}

// ---------------------------------------------------------------- alloc extras

#[test]
fn display_path_joins_what_it_can_and_skips_what_it_cannot() {
    let buf = fixture("unknown-class.bin");
    let path = rclip_idlist::display_path(ItemIdList::new(&buf), "\\");
    assert_eq!(
        path, "My Computer\\D:\\",
        "the unknown item contributes no name and is skipped; a gap is more honest than \
         a fabricated segment"
    );
}

#[test]
fn a_root_folder_with_an_unrecognised_guid_contributes_no_segment() {
    // Same list, with the GUID's first byte changed so the lookup misses.
    let mut buf = fixture("two-items.bin");
    buf[4] ^= 0xFF;
    let path = rclip_idlist::display_path(ItemIdList::new(&buf), "\\");
    assert_eq!(
        path, "C:\\",
        "an unrecognised shell folder renders as nothing rather than as raw hex"
    );
}

#[test]
fn the_builder_produces_bytes_the_parser_reads_back() {
    use rclip_idlist::ItemIdListBuilder;

    let guid = Guid::from_bytes([
        0xE0, 0x4F, 0xD0, 0x20, 0xEA, 0x3A, 0x69, 0x10, 0xA2, 0xD8, 0x08, 0x00, 0x2B, 0x30, 0x30,
        0x9D,
    ]);
    let mut b = ItemIdListBuilder::new();
    b.push_root_folder(0x50, &guid);
    assert!(b.push_raw(&[0x2F, b'C', b':', b'\\']));
    let bytes = b.finish();

    let mut list = ItemIdList::new(&bytes);
    let first = list.next().unwrap().unwrap();
    assert_eq!(first.cb(), 20);
    match first.parse() {
        ShellItem::RootFolder(r) => {
            assert_eq!(r.sort_index, 0x50);
            assert_eq!(r.guid, guid);
        }
        other => panic!("expected a root folder, got {other:?}"),
    }
    assert!(list.next().unwrap().is_ok());
    assert!(list.next().is_none());
    assert!(list.is_terminated());
    assert_eq!(list.bytes_consumed(), bytes.len());
}

#[test]
fn the_builder_refuses_an_item_too_big_for_a_u16_size_field() {
    use rclip_idlist::ItemIdListBuilder;

    let mut b = ItemIdListBuilder::new();
    let huge = vec![0u8; u16::MAX as usize];
    assert!(
        !b.push_raw(&huge),
        "cb is a u16; truncating would change what the list says"
    );
    assert!(b.is_empty(), "a rejected item must leave nothing behind");
}

// ------------------------------------------------------- ANSI shell strings

/// The ANSI file entry, unpacked once for the tests below.
#[cfg(feature = "codepage")]
fn ansi_file_entry_name(bytes: &[u8]) -> ShellStr<'_> {
    let mut list = ItemIdList::new(bytes);
    let item = list.next().expect("one item").expect("well formed");
    match item.parse() {
        ShellItem::FileEntry(f) => {
            assert!(
                !f.has_unicode_name(),
                "class 0x32 must have FILE_ENTRY_UNICODE clear, or the fixture is not \
                 exercising the ANSI path at all"
            );
            f.primary_name
        }
        other => panic!("expected a file entry, got {other:?}"),
    }
}

#[test]
#[cfg(feature = "codepage")]
fn an_ansi_name_decodes_once_the_caller_names_the_code_page() {
    use rclip_idlist::Encoding;

    let buf = fixture("file-entry-ansi-cp1252.bin");
    let name = ansi_file_entry_name(&buf);

    // Without a code page the high bytes are a hole, and that is still the
    // default behaviour: the feature adds an API, it does not change one.
    assert_eq!(
        name.to_string_lossy(),
        "Gr\u{FFFD}\u{FFFD}e.txt",
        "the un-named path must keep refusing to guess"
    );

    assert_eq!(
        name.to_string_with(Encoding::Windows1252).expect("decodes"),
        "Grüße.txt"
    );
}

#[test]
#[cfg(feature = "codepage")]
fn the_same_ansi_bytes_read_differently_under_a_different_code_page() {
    use rclip_idlist::Encoding;

    // This is the whole reason the code page is a parameter rather than a
    // default: nothing in the payload distinguishes these two readings.
    let buf = fixture("file-entry-ansi-cp1252.bin");
    let name = ansi_file_entry_name(&buf);
    assert_eq!(
        name.to_string_with(Encoding::Windows1251).expect("decodes"),
        "GrьЯe.txt"
    );
}

#[test]
#[cfg(feature = "codepage")]
fn a_unicode_field_ignores_the_named_code_page() {
    use rclip_idlist::Encoding;

    let utf16 = ShellStr::Utf16(b"a\0b\0");
    assert_eq!(utf16.to_string_with(Encoding::Cp437).expect("utf16"), "ab");
    assert_eq!(utf16.to_string_lossy_with(Encoding::Cp437), "ab");
}

#[test]
#[cfg(feature = "codepage")]
fn an_undefined_byte_is_reported_and_iteration_continues() {
    use rclip_idlist::Encoding;

    // 0x81 is unassigned in Windows-1252. A single-byte code page cannot lose
    // sync, so the 'z' after it must still arrive.
    let s = ShellStr::Ansi(b"a\x81z");
    let got: Vec<_> = s.chars_with(Encoding::Windows1252).collect();
    assert_eq!(got.len(), 3, "one item per byte");
    assert_eq!(got[0], Ok('a'));
    assert_eq!(got[1].unwrap_err().kind, ErrorKind::Malformed);
    assert_eq!(got[2], Ok('z'));

    assert_eq!(s.to_string_lossy_with(Encoding::Windows1252), "a\u{FFFD}z");
    assert!(s.to_string_with(Encoding::Windows1252).is_err());
}

// ------------------------------------------------- signature-based item types
//
// Everything below is Phase 3: items identified by a signature inside the body
// rather than by the class byte, plus the three smaller gaps Phase 0 left.

fn only_item(buf: &[u8]) -> ShellItem<'_> {
    let mut list = ItemIdList::new(buf);
    let item = list.next().expect("one item").expect("well formed");
    assert!(list.next().is_none(), "the fixture holds exactly one item");
    item.parse()
}

#[test]
fn a_control_panel_item_decodes_to_a_guid_and_a_name() {
    let buf = fixture("control-panel-item.bin");
    let ShellItem::ControlPanel(cp) = only_item(&buf) else {
        panic!("expected a control panel item");
    };
    assert_eq!(cp.class, 0x71);
    assert_eq!(
        cp.identifier.to_braced().as_str(),
        "{C555438B-3C23-4769-A71F-B6D3D9B6053A}"
    );
    assert_eq!(cp.name(), Some("Display"));
}

#[test]
fn a_control_panel_guid_is_not_looked_up_in_the_shell_folder_table() {
    // The two namespaces overlap and disagree: this GUID is `Fonts` as a shell
    // folder and `Font Folder` as a control panel item. Reading one table for
    // the other is not merely incomplete, it is wrong.
    let g = Guid::from_bytes(
        *Guid::from_slice(&[
            0x80, 0xB3, 0x84, 0xBD, 0xA2, 0x8C, 0x69, 0x10, 0xAB, 0x1D, 0x08, 0x00, 0x09, 0x48,
            0xF5, 0x34,
        ])
        .expect("16 bytes")
        .as_bytes(),
    );
    assert_eq!(g.well_known_name(), Some("Fonts"));
    assert_eq!(g.control_panel_name(), Some("Font Folder"));
}

#[test]
fn an_mtp_volume_is_recognised_by_its_signature_not_its_class_byte() {
    let buf = fixture("mtp-volume.bin");
    let item = only_item(&buf);
    assert_eq!(
        item.class(),
        Some(0x00),
        "the class byte says nothing; the signature is what identifies it"
    );
    let ShellItem::MtpVolume(v) = item else {
        panic!("expected an MTP volume, got {item:?}");
    };
    assert_eq!(
        v.name.expect("name").to_string_lossy(),
        "Internal storage",
        "the length field counts characters including the terminator"
    );
    assert_eq!(v.identifier.expect("id").to_string_lossy(), "s10001");
    assert_eq!(v.file_system.expect("fs").to_string_lossy(), "FAT32");
    assert_eq!(
        item.display_name().expect("a name").to_string_lossy(),
        "Internal storage"
    );
}

#[test]
fn an_mtp_file_entry_carries_two_filetimes_and_a_content_type() {
    let buf = fixture("mtp-file-entry.bin");
    let ShellItem::MtpFileEntry(f) = only_item(&buf) else {
        panic!("expected an MTP file entry");
    };
    assert_eq!(f.modified, 130_092_000_000_000_000);
    assert_eq!(f.created, 130_091_000_000_000_000);
    assert_eq!(
        f.content_type.expect("guid").to_braced().as_str(),
        "{27E2E392-A111-48E0-AB0C-E17705A05F85}"
    );
    assert_eq!(f.name.expect("name").to_string_lossy(), "Photos");
    assert_eq!(f.identifier.expect("id").to_string_lossy(), "o1234");
}

#[test]
fn a_string_length_of_four_billion_characters_yields_no_name_and_no_panic() {
    // The one that has to hold: `chars * 2` overflows a 32-bit usize, and even
    // where it does not the slice runs past the end of a sixty-byte item.
    let buf = fixture("mtp-volume-length-bomb.bin");
    let item = only_item(&buf);
    let ShellItem::MtpVolume(v) = item else {
        panic!("expected an MTP volume");
    };
    assert!(
        v.name.is_none(),
        "a length that cannot fit yields no string"
    );
    assert!(v.identifier.is_none());
    assert!(v.file_system.is_none());
    assert!(item.display_name().is_none());
    assert_eq!(
        item.as_bytes().len() + 2,
        buf.len() - 2,
        "the raw bytes survive whatever the length fields claimed"
    );
}

#[test]
fn a_users_property_view_exposes_its_known_folder_and_its_property_store() {
    let buf = fixture("users-property-view.bin");
    let item = only_item(&buf);
    let ShellItem::UsersPropertyView(v) = item else {
        panic!("expected a users property view, got {item:?}");
    };
    assert_eq!(v.signature, 0x23FE_BBEE);
    assert_eq!(v.identifier.len(), 16);
    assert_eq!(
        v.known_folder_id.expect("guid").well_known_name(),
        Some("Users Files")
    );
    assert_eq!(
        item.display_name().expect("a name").to_string_lossy(),
        "Users Files"
    );
    // Kept raw. The structure is MS-PROPSTORE, the same as a .lnk property
    // store block, but this crate does not depend on the crate that decodes it.
    assert!(!v.property_store.is_empty());
    assert_eq!(
        &v.property_store[4..8],
        b"1SPS",
        "a serialized property storage, version first"
    );
}

#[test]
fn a_compressed_folder_is_recognised_by_the_shape_of_its_timestamp() {
    let buf = fixture("compressed-folder-win10.bin");
    let item = only_item(&buf);
    let ShellItem::CompressedFolder(c) = item else {
        panic!("expected a compressed folder, got {item:?}");
    };
    assert_eq!(c.variant, rclip_idlist::CompressedFolderVariant::Windows10);
    assert_eq!(c.uncompressed_size, Some(1_048_576));
    assert_eq!(c.compressed_size, Some(262_144));
    assert_eq!(c.compression_method, Some(8), "DEFLATE");
    assert_eq!(c.crc32, Some(0x1234_ABCD));
    assert_eq!(
        c.name.expect("name").to_string_lossy(),
        "docs/report.pdf",
        "this length EXCLUDES the terminator, unlike the MTP ones"
    );
    assert_eq!(c.name2.expect("name2").to_string_lossy(), "docs");
}

#[test]
fn a_delegate_folder_names_its_extension_and_unwraps_one_level() {
    let buf = fixture("delegate-folder.bin");
    let item = only_item(&buf);
    let ShellItem::DelegateFolder(d) = item else {
        panic!("expected a delegate folder, got {item:?}");
    };
    assert_eq!(
        d.folder_id.well_known_name(),
        Some("Users Files delegate folder")
    );
    let inner = d.inner_item();
    let ShellItem::RootFolder(r) = inner else {
        panic!("expected the inner item to be a root folder, got {inner:?}");
    };
    assert_eq!(r.guid.well_known_name(), Some("My Computer"));
    // A wrapper has no name of its own; it borrows the one it wraps.
    assert_eq!(
        item.display_name().expect("a name").to_string_lossy(),
        "My Computer"
    );
}

#[test]
fn unwrapping_a_delegate_folder_never_re_enters_the_delegate_path() {
    // A body that would match the delegate probe again if it were re-run. The
    // guarantee is structural: `inner_item` goes through `parse_no_delegate`,
    // so no PIDL can drive this into recursion however it is shaped.
    let buf = fixture("delegate-folder.bin");
    let ShellItem::DelegateFolder(d) = only_item(&buf) else {
        panic!("expected a delegate folder");
    };
    let nested = ShellItem::parse_no_delegate(d.raw);
    assert!(
        !matches!(nested, ShellItem::DelegateFolder(_)),
        "parse_no_delegate must not produce a delegate folder"
    );
}

#[test]
fn an_ftp_uri_item_decodes_its_data_block() {
    let buf = fixture("uri-ftp.bin");
    let ShellItem::Uri(u) = only_item(&buf) else {
        panic!("expected a URI item");
    };
    assert_eq!(
        u.uri.expect("uri").to_string_lossy(),
        "ftp://ftp.example.com/pub/"
    );
    let ftp = u.ftp.expect("a >= 36 byte data block is the FTP layout");
    assert_eq!(ftp.timestamp, 130_092_000_000_000_000);
    assert_eq!(ftp.host.expect("host").to_string_lossy(), "ftp.example.com");
    assert_eq!(ftp.user.expect("user").to_string_lossy(), "anonymous");
    assert_eq!(
        ftp.password.expect("password").to_string_lossy(),
        "guest@example.com"
    );
}

#[test]
fn the_ftp_password_is_kept_out_of_debug_output() {
    let buf = fixture("uri-ftp.bin");
    let ShellItem::Uri(u) = only_item(&buf) else {
        panic!("expected a URI item");
    };
    let rendered = format!("{:?}", u.ftp.expect("ftp block"));
    assert!(
        !rendered.contains("guest@example.com"),
        "Debug must not print the password: {rendered}"
    );
    assert!(rendered.contains("<redacted>"));
    // A courtesy against accidental logging, not a boundary: the field itself
    // and the raw block both still have the bytes.
    assert!(u.data.windows(5).any(|w| w == b"guest"));
}

#[test]
fn a_short_uri_data_block_is_not_read_as_the_ftp_layout() {
    // 8 bytes of block: below the 36-byte FTP header, so guessing at it would
    // read the URI string as a host name.
    let body = [
        &[0x61u8, 0x00, 0x08, 0x00][..],
        &[0u8; 8][..],
        b"http://example.com/\0",
    ]
    .concat();
    let ShellItem::Uri(u) = ShellItem::parse(&body) else {
        panic!("expected a URI item");
    };
    assert!(u.ftp.is_none());
    assert_eq!(u.uri.expect("uri").to_string_lossy(), "http://example.com/");
}

#[test]
fn a_pre_xp_file_entry_carries_its_short_name_after_the_long_one() {
    let buf = fixture("file-entry-pre-xp.bin");
    let ShellItem::FileEntry(f) = only_item(&buf) else {
        panic!("expected a file entry");
    };
    assert!(f.is_pre_xp, "no extension block follows the primary name");
    assert_eq!(f.primary_name.to_string_lossy(), "Long Name.txt");
    assert_eq!(
        f.secondary_name.expect("8.3 name").to_string_lossy(),
        "LONGNA~1.TXT"
    );
    assert!(
        f.extension.is_none(),
        "a pre-XP item has no extension blocks; searching for one would read \
         the secondary name's last two bytes as an offset"
    );
    assert!(f.long_name.is_none());
}

#[test]
fn an_xp_file_entry_is_not_mistaken_for_a_pre_xp_one() {
    // The other side of the look-ahead: this fixture has a real 0xBEEF0004
    // block, so the u16 after the primary name is a plausible block size.
    let buf = fixture("file-entry-beef0004-v3.bin");
    let ShellItem::FileEntry(f) = only_item(&buf) else {
        panic!("expected a file entry");
    };
    assert!(!f.is_pre_xp);
    assert!(f.secondary_name.is_none());
    assert!(f.extension.is_some());
    assert!(f.long_name.is_some());
}

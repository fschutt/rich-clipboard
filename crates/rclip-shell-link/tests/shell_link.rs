//! Integration tests for `rclip-shell-link`.
//!
//! Weighted towards the three things a `.lnk` parser gets wrong: the
//! character-vs-byte `StringData` count, the offset bases inside `LinkInfo`, and
//! an `ExtraData` walk that can be made to stall or over-read.

use rclip_core::ErrorKind;
use rclip_shell_link::{
    extra::{self, ExtraDataBlock},
    header::{FileAttributes, HotKey, LinkFlags, ShowCommand},
    link_info::DriveType,
    FileTime, ShellLink, ShellStr,
};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-shell-link/"
    );
    std::fs::read(format!("{p}{name}")).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

// ---------------------------------------------------------------- header

#[test]
fn a_header_only_link_has_no_optional_sections() {
    let buf = fixture("minimal-header-only.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    assert!(link.header.link_flags.is_empty());
    assert!(link.target_id_list.is_none());
    assert!(link.link_info.is_none());
    assert!(link.string_data.is_empty());
    assert!(
        link.header.hot_key.is_unset(),
        "no hotkey is the normal case, not an error"
    );
    assert!(link.header.creation_time.is_unset());
    assert_eq!(link.extra_data().count(), 0);
}

#[test]
fn a_truncated_header_is_rejected() {
    let err = ShellLink::parse(&fixture("truncated-header.bin")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnexpectedEof);
}

#[test]
fn a_wrong_clsid_is_bad_magic() {
    let err = ShellLink::parse(&fixture("bad-clsid.bin")).unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::BadMagic,
        "the CLSID is the only thing that says these bytes are a shell link at all"
    );
    assert_eq!(err.offset, 4, "the offset points at the CLSID field");
}

#[test]
fn a_wrong_header_size_is_bad_length() {
    let err = ShellLink::parse(&fixture("bad-header-size.bin")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLength);
    assert_eq!(err.offset, 0);
}

#[test]
fn an_empty_input_does_not_panic() {
    assert!(ShellLink::parse(&[]).is_err());
    assert!(ShellLink::parse(&[0u8; 1]).is_err());
}

#[test]
fn undefined_link_flag_bits_are_kept_rather_than_rejected() {
    // Bit 31 is undefined in MS-SHLLINK 10.0. A future Windows setting it must
    // not make the file unreadable.
    let mut buf = fixture("minimal-header-only.bin");
    buf[20..24].copy_from_slice(&0x8000_0000u32.to_le_bytes());

    let link = ShellLink::parse(&buf).expect("an undefined flag bit is not a parse failure");
    assert_eq!(link.header.link_flags.unknown_bits(), 0x8000_0000);
}

#[test]
fn an_unrecognised_show_command_reads_back_as_normal_without_losing_the_raw_value() {
    let mut buf = fixture("minimal-header-only.bin");
    buf[60..64].copy_from_slice(&0x1234u32.to_le_bytes());

    let link = ShellLink::parse(&buf).unwrap();
    assert_eq!(
        link.header.show_command.0, 0x1234,
        "the raw value survives for round-tripping"
    );
    assert_eq!(
        link.header.show_command.effective(),
        ShowCommand::NORMAL,
        "MS-SHLLINK 2.1: all other values MUST be treated as SW_SHOWNORMAL"
    );
}

#[test]
fn the_header_round_trips_through_to_bytes() {
    let buf = fixture("full-featured.bin");
    let link = ShellLink::parse(&buf).unwrap();
    assert_eq!(link.header.to_bytes()[..], buf[..76]);
}

#[test]
fn hot_key_modifiers_and_key_decode() {
    let buf = fixture("full-featured.bin");
    let hk = ShellLink::parse(&buf).unwrap().header.hot_key;
    assert!(!hk.is_unset());
    assert_eq!(hk.key_char(), Some('A'));
    assert!(hk.has_control());
    assert!(hk.has_alt());
    assert!(!hk.has_shift());
    assert_eq!(hk.function_key(), None);
    assert_eq!(
        HotKey {
            key: 0x7B,
            modifiers: 0
        }
        .function_key(),
        Some(12),
        "VK_F12"
    );
}

#[test]
fn a_negative_icon_index_stays_negative() {
    let buf = fixture("full-featured.bin");
    let link = ShellLink::parse(&buf).unwrap();
    assert_eq!(
        link.header.icon_index, -3,
        "IconIndex is signed; a negative value is a resource ID, not an index"
    );
}

#[test]
fn link_flags_debug_names_the_bits_it_knows_and_flags_the_ones_it_does_not() {
    let f = LinkFlags(LinkFlags::HAS_NAME.0 | LinkFlags::IS_UNICODE.0 | 0x8000_0000);
    let s = format!("{f:?}");
    assert!(s.contains("HasName"), "{s}");
    assert!(s.contains("IsUnicode"), "{s}");
    assert!(s.contains("undefined"), "{s}");
}

// ---------------------------------------------------------------- StringData

#[test]
fn unicode_string_data_counts_characters_not_bytes() {
    let buf = fixture("with-string-data.bin");
    let link = ShellLink::parse(&buf).expect("well formed");
    let sd = link.string_data;

    // If the count were read as a byte length every field after the first would
    // be misaligned, so getting all five back in order is the real assertion.
    assert_eq!(sd.name.unwrap().to_string_lossy(), "My Notes");
    assert_eq!(sd.relative_path.unwrap().to_string_lossy(), ".\\notes.txt");
    assert_eq!(sd.working_dir.unwrap().to_string_lossy(), "C:\\Users\\me");
    assert_eq!(sd.arguments.unwrap().to_string_lossy(), "--flag \"a b\"");
    assert_eq!(
        sd.icon_location.unwrap().to_string_lossy(),
        "%SystemRoot%\\system32\\shell32.dll"
    );
    assert!(matches!(sd.name.unwrap(), ShellStr::Utf16(_)));
}

#[test]
fn ansi_string_data_counts_single_bytes() {
    let buf = fixture("ansi-string-data.bin");
    let sd = ShellLink::parse(&buf).expect("well formed").string_data;

    assert_eq!(sd.name.unwrap().as_ascii(), Some("Plain"));
    assert_eq!(sd.relative_path.unwrap().as_ascii(), Some(".\\a.txt"));
    assert!(matches!(sd.name.unwrap(), ShellStr::Ansi(_)));
    assert_eq!(sd.working_dir, None, "a clear flag means absent, not empty");
}

#[test]
fn a_string_count_the_input_cannot_back_is_an_error_not_a_short_read() {
    let err = ShellLink::parse(&fixture("string-count-past-end.bin")).unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::UnexpectedEof,
        "65535 characters is 131070 bytes; computing that in u16 wraps to 65534 and \
         silently produces a short string instead of this error"
    );
}

#[test]
fn a_string_field_absent_and_a_string_field_empty_are_different_things() {
    // HasName set, CountCharacters zero.
    let mut buf = fixture("minimal-header-only.bin");
    buf[20..24].copy_from_slice(&(LinkFlags::HAS_NAME.0 | LinkFlags::IS_UNICODE.0).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());

    let sd = ShellLink::parse(&buf).unwrap().string_data;
    let name = sd.name.expect("present");
    assert!(name.is_empty());
    assert_eq!(sd.relative_path, None);
}

// ---------------------------------------------------------------- LinkInfo

#[test]
fn link_info_offsets_are_measured_from_the_structure_and_not_the_file() {
    let buf = fixture("full-featured.bin");
    let link = ShellLink::parse(&buf).expect("well formed");
    let info = link.link_info.expect("HasLinkInfo is set");

    assert_eq!(
        info.header_size, 0x1C,
        "ASCII-only, so no Unicode offset fields"
    );
    assert_eq!(info.local_base_path_offset_unicode, None);

    let path = info
        .local_base_path()
        .unwrap()
        .expect("VolumeIDAndLocalBasePath is set");
    assert_eq!(
        path.as_ascii(),
        Some("C:\\Users\\me\\notes.txt"),
        "a wrong offset base yields a string from the middle of another field rather \
         than an error, so this is the assertion that catches it"
    );
    assert_eq!(info.common_path_suffix().unwrap().as_ascii(), Some(""));

    let vol = info.volume_id().unwrap().expect("VolumeID present");
    assert_eq!(vol.drive_type, DriveType::FIXED);
    assert_eq!(vol.drive_type.name(), Some("DRIVE_FIXED"));
    assert_eq!(vol.drive_serial_number, 0x1234_ABCD);
    assert_eq!(vol.volume_label().unwrap().as_ascii(), Some("OS"));
    assert!(
        vol.size > 0x10,
        "MS-SHLLINK 2.3.1: VolumeIDSize MUST be greater than 0x10"
    );
}

#[test]
fn a_network_link_info_yields_the_unc_path_and_the_mapped_drive() {
    let buf = fixture("link-info-network.bin");
    let link = ShellLink::parse(&buf).expect("well formed");
    let info = link.link_info.expect("present");

    assert!(
        info.volume_id().unwrap().is_none(),
        "no VolumeIDAndLocalBasePath flag"
    );
    assert!(info.local_base_path().unwrap().is_none());

    let net = info
        .common_network_relative_link()
        .unwrap()
        .expect("present");
    assert_eq!(
        net.net_name().unwrap().as_ascii(),
        Some("\\\\fileserver\\public")
    );
    assert_eq!(net.device_name().unwrap().unwrap().as_ascii(), Some("Z:"));
    assert_eq!(net.network_provider_type.name(), Some("DAV"));
    assert_eq!(
        net.net_name_offset_unicode, None,
        "NetNameOffset is exactly 0x14, so the Unicode offset fields are absent"
    );
}

#[test]
fn the_unassigned_wnnc_gap_has_no_name() {
    use rclip_shell_link::link_info::NetworkProviderType;
    assert_eq!(NetworkProviderType(0x001A_0000).name(), Some("AVID"));
    assert_eq!(NetworkProviderType(0x0043_0000).name(), Some("GOOGLE"));
    assert_eq!(
        NetworkProviderType(0x0028_0000).name(),
        None,
        "MS-SHLLINK's table skips 0x00280000; that gap is in the spec, not a transcription slip"
    );
    assert_eq!(NetworkProviderType(0).name(), None);
    assert_eq!(NetworkProviderType(0xFFFF_FFFF).name(), None);
}

#[test]
fn a_link_info_too_small_for_its_own_header_is_rejected() {
    let err = ShellLink::parse(&fixture("link-info-too-small.bin")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLength);
}

// ---------------------------------------------------------------- target IDList

#[test]
fn the_target_id_list_walks_through_to_rclip_idlist() {
    use rclip_idlist::ShellItem;

    let buf = fixture("full-featured.bin");
    let link = ShellLink::parse(&buf).expect("well formed");
    let target = link.target_id_list.expect("HasLinkTargetIDList is set");

    let items: Vec<_> = target.items().map(|i| i.expect("well formed")).collect();
    assert_eq!(items.len(), 3);
    assert!(matches!(items[0].parse(), ShellItem::RootFolder(_)));
    assert!(matches!(items[1].parse(), ShellItem::Volume(_)));

    match items[2].parse() {
        ShellItem::FileEntry(f) => {
            assert_eq!(f.long_name.unwrap().to_string_lossy(), "notes.txt");
        }
        other => panic!("expected a file entry, got {other:?}"),
    }
    assert_eq!(
        target.wire_size(),
        2 + target.id_list_size as usize,
        "IDListSize counts the IDList including its terminator, not the size field"
    );
}

#[test]
fn an_id_list_size_past_the_end_is_caught_before_the_walk_starts() {
    let err = ShellLink::parse(&fixture("id-list-size-past-end.bin")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnexpectedEof);
}

// ---------------------------------------------------------------- ExtraData

#[test]
fn a_single_environment_variable_block_parses() {
    let buf = fixture("with-extra-data.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    let blocks: Vec<_> = link.extra_data().map(|b| b.expect("well formed")).collect();
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ExtraDataBlock::EnvironmentVariable(p) => {
            assert_eq!(p.path().to_string_lossy(), "%windir%\\system32\\cmd.exe");
            assert_eq!(
                p.ansi, b"%windir%\\system32\\cmd.exe",
                "the ANSI half is trimmed at its NUL, not padded out to 260"
            );
        }
        other => panic!("expected an environment variable block, got {other:?}"),
    }
    assert_eq!(
        link.environment_path().unwrap().to_string_lossy(),
        "%windir%\\system32\\cmd.exe"
    );
}

#[test]
fn every_extra_data_block_in_the_full_fixture_parses() {
    let buf = fixture("full-featured.bin");
    let link = ShellLink::parse(&buf).expect("well formed");
    let blocks: Vec<_> = link.extra_data().map(|b| b.expect("well formed")).collect();
    assert_eq!(blocks.len(), 9);

    let sigs: Vec<u32> = blocks.iter().map(ExtraDataBlock::signature).collect();
    assert_eq!(
        sigs,
        [
            extra::SIG_ENVIRONMENT_VARIABLE,
            extra::SIG_ICON_ENVIRONMENT,
            extra::SIG_CONSOLE,
            extra::SIG_CONSOLE_FE,
            extra::SIG_SPECIAL_FOLDER,
            extra::SIG_KNOWN_FOLDER,
            extra::SIG_TRACKER,
            extra::SIG_VISTA_AND_ABOVE_ID_LIST,
            0xA000_000A,
        ]
    );

    match &blocks[2] {
        ExtraDataBlock::Console(c) => {
            assert_eq!(c.face_name.to_string_lossy(), "Consolas");
            assert_eq!(c.screen_buffer_size_x, 120);
            assert_eq!(c.window_size_y, 30);
            assert!(!c.is_bold(), "weight 400 is regular");
            assert!(c.quick_edit);
            assert!(!c.full_screen);
            assert_eq!(c.color_table[15], 15);
        }
        other => panic!("expected a console block, got {other:?}"),
    }
    assert!(matches!(
        blocks[3],
        ExtraDataBlock::ConsoleFe { code_page: 932 }
    ));
    assert!(matches!(
        blocks[4],
        ExtraDataBlock::SpecialFolder {
            special_folder_id: 0x28,
            offset: 0x14
        }
    ));
    match &blocks[5] {
        ExtraDataBlock::KnownFolder {
            known_folder_id,
            offset,
        } => {
            assert_eq!(known_folder_id.well_known_name(), Some("Documents"));
            assert_eq!(*offset, 0x14);
        }
        other => panic!("expected a known folder block, got {other:?}"),
    }
    match &blocks[6] {
        ExtraDataBlock::Tracker(t) => {
            assert_eq!(t.length, 0x58, "MS-SHLLINK 10.0 fixes Length at 0x58");
            assert_eq!(t.version, 0);
            assert_eq!(t.machine_id.as_ascii(), Some("WORKSTATION-01"));
            assert_ne!(t.droid[0], t.droid_birth[0]);
        }
        other => panic!("expected a tracker block, got {other:?}"),
    }
}

#[test]
fn an_unassigned_signature_round_trips_as_unknown_instead_of_failing() {
    let buf = fixture("full-featured.bin");
    let link = ShellLink::parse(&buf).unwrap();
    let unknown = link.find_extra(0xA000_000A).expect("the block is there");
    match unknown {
        ExtraDataBlock::Unknown {
            signature,
            size,
            body,
        } => {
            assert_eq!(
                signature, 0xA000_000A,
                "unassigned in MS-SHLLINK 10.0 — for now"
            );
            assert_eq!(size as usize, body.len() + 8);
            assert_eq!(body, b"reserved-signature-body");
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn the_vista_and_above_id_list_block_has_no_leading_size_field() {
    let buf = fixture("full-featured.bin");
    let link = ShellLink::parse(&buf).unwrap();
    let block = link
        .find_extra(extra::SIG_VISTA_AND_ABOVE_ID_LIST)
        .expect("present");
    let list = block.id_list().expect("this variant carries one");
    assert_eq!(
        list.count(),
        3,
        "unlike LinkTargetIDList, this block's IDList starts immediately after the \
         signature — reading a u16 size first would eat the first item's cb"
    );
}

#[test]
fn an_extra_block_too_small_to_hold_a_signature_is_rejected_rather_than_skipped() {
    let buf = fixture("extra-block-too-small.bin");
    let link = ShellLink::parse(&buf).expect("the header and StringData are fine");

    let mut blocks = link.extra_data();
    let err = blocks.next().expect("an error").unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::BadLength,
        "a BlockSize of 6 is past the terminal threshold but below the 8 bytes a size \
         and signature need; advancing by it would land inside the next block"
    );
    assert!(blocks.next().is_none(), "the walk yields at most one error");
}

#[test]
fn a_block_declaring_more_than_remains_is_eof() {
    let mut buf = fixture("with-extra-data.bin");
    let start = buf.len() - 4 - 0x314;
    buf[start..start + 4].copy_from_slice(&0x0000_FFFFu32.to_le_bytes());

    let link = ShellLink::parse(&buf).unwrap();
    let err = link.extra_data().next().unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnexpectedEof);
}

#[test]
fn a_block_whose_declared_size_disagrees_with_its_signature_is_rejected() {
    // The fixed-size blocks have their fields at fixed offsets. A block that is
    // the wrong size is a block whose fields are not where they should be, so
    // reading it would produce confident nonsense.
    let mut buf = fixture("with-extra-data.bin");
    let start = buf.len() - 4 - 0x314;
    buf[start..start + 4].copy_from_slice(&0x0000_0100u32.to_le_bytes());
    buf.truncate(start + 0x100);
    buf.extend_from_slice(&0u32.to_le_bytes());

    let link = ShellLink::parse(&buf).unwrap();
    let err = link.extra_data().next().unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLength);
}

#[test]
fn the_extra_data_walk_terminates_on_arbitrary_bytes() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for round in 0..2000 {
        let len = (next() % 160) as usize;
        let tail: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();

        let mut buf = fixture("minimal-header-only.bin");
        buf.extend_from_slice(&tail);

        let link = ShellLink::parse(&buf).expect("the header is intact");
        let mut steps = 0usize;
        for block in link.extra_data() {
            steps += 1;
            assert!(
                steps <= tail.len() + 1,
                "round {round} failed to make progress"
            );
            match block {
                Ok(b) => {
                    let _ = b.signature();
                    if let Some(list) = b.id_list() {
                        let _ = list.take(64).count();
                    }
                }
                Err(_) => break,
            }
        }
    }
}

#[test]
fn whole_file_fuzz_never_panics() {
    let seeds = [
        "full-featured.bin",
        "with-string-data.bin",
        "link-info-network.bin",
        "with-extra-data.bin",
    ];
    let mut state = 0xD1B5_4A32_D192_ED03u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for seed in seeds {
        let original = fixture(seed);
        for _ in 0..500 {
            let mut buf = original.clone();
            // Flip a handful of bytes anywhere after the CLSID, so the file
            // still identifies as a shell link and the deeper parsers are
            // actually reached.
            for _ in 0..4 {
                let i = 20 + (next() as usize % (buf.len() - 20));
                buf[i] ^= (next() & 0xFF) as u8;
            }
            // Truncation is its own failure mode.
            if next() % 4 == 0 {
                let keep = next() as usize % buf.len();
                buf.truncate(keep);
            }

            if let Ok(link) = ShellLink::parse(&buf) {
                let _ = link.string_data.name.map(|s| s.to_string_lossy());
                if let Some(info) = &link.link_info {
                    let _ = info.local_base_path();
                    let _ = info.volume_id().map(|v| v.map(|v| v.volume_label()));
                    let _ = info
                        .common_network_relative_link()
                        .map(|n| n.map(|n| n.net_name()));
                    let _ = info.common_path_suffix();
                }
                if let Some(t) = &link.target_id_list {
                    for item in t.items().take(256) {
                        let Ok(item) = item else { break };
                        let _ = item.parse().display_name();
                    }
                }
                let mut steps = 0usize;
                for block in link.extra_data() {
                    steps += 1;
                    assert!(
                        steps <= buf.len(),
                        "extra data walk failed to make progress"
                    );
                    if block.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- FILETIME

#[test]
fn filetime_converts_to_unix_seconds_without_a_date_library() {
    let buf = fixture("full-featured.bin");
    let t = ShellLink::parse(&buf).unwrap().header.creation_time;
    assert!(!t.is_unset());
    // 130092000000000000 ticks past 1601-01-01 is 2013-03-31T12:00:00Z, i.e.
    // (130092000000000000 - 116444736000000000) / 10_000_000 seconds past 1970.
    assert_eq!(t.unix_seconds(), Some(1_364_726_400));
    assert_eq!(FileTime::from_unix_seconds(1_364_726_400), t, "round trips");
}

#[test]
fn a_zero_filetime_is_unset_not_the_year_1601() {
    assert!(FileTime(0).is_unset());
    assert_eq!(FileTime(0).unix_seconds(), None);
}

#[test]
fn a_pre_1970_filetime_stays_negative_rather_than_saturating() {
    // 1980-01-01T00:00:00Z.
    let t = FileTime::from_unix_seconds(315_532_800);
    assert_eq!(t.unix_seconds(), Some(315_532_800));
    // 1900 predates the Unix epoch but not the FILETIME epoch.
    let early = FileTime::from_unix_seconds(-2_208_988_800);
    assert_eq!(early.unix_seconds(), Some(-2_208_988_800));
}

// ---------------------------------------------------------------- writer

#[test]
fn a_written_link_parses_back_to_what_went_in() {
    use rclip_shell_link::ShellLinkBuilder;

    let bytes = ShellLinkBuilder::new()
        .name("My Notes")
        .relative_path(".\\notes.txt")
        .working_dir("C:\\Users\\me")
        .arguments("--verbose")
        .icon_location("%SystemRoot%\\system32\\shell32.dll")
        .icon_index(-3)
        .local_path("C:\\Users\\me\\notes.txt")
        .volume(DriveType::FIXED, 0x1234_ABCD, "OS")
        .environment_path("%USERPROFILE%\\notes.txt")
        .show_command(ShowCommand::MAXIMIZED)
        .hot_key(HotKey {
            key: b'A',
            modifiers: HotKey::CONTROL | HotKey::ALT,
        })
        .file_attributes(FileAttributes::ARCHIVE)
        .times(FileTime(130_092_000_000_000_000), FileTime(0), FileTime(0))
        .build()
        .expect("all fields are within the format's limits");

    let link = ShellLink::parse(&bytes).expect("what we wrote must be what we can read");

    assert!(link.header.link_flags.is_unicode());
    assert_eq!(link.header.icon_index, -3);
    assert_eq!(link.header.show_command, ShowCommand::MAXIMIZED);
    assert_eq!(link.header.hot_key.key_char(), Some('A'));
    assert!(link
        .header
        .file_attributes
        .contains(FileAttributes::ARCHIVE));
    assert_eq!(link.header.creation_time.0, 130_092_000_000_000_000);

    let sd = link.string_data;
    assert_eq!(sd.name.unwrap().to_string_lossy(), "My Notes");
    assert_eq!(sd.relative_path.unwrap().to_string_lossy(), ".\\notes.txt");
    assert_eq!(sd.working_dir.unwrap().to_string_lossy(), "C:\\Users\\me");
    assert_eq!(sd.arguments.unwrap().to_string_lossy(), "--verbose");
    assert_eq!(
        sd.icon_location.unwrap().to_string_lossy(),
        "%SystemRoot%\\system32\\shell32.dll"
    );

    let info = link
        .link_info
        .as_ref()
        .expect("local_path implies a LinkInfo");
    assert_eq!(
        info.local_base_path().unwrap().unwrap().as_ascii(),
        Some("C:\\Users\\me\\notes.txt")
    );
    let vol = info.volume_id().unwrap().unwrap();
    assert_eq!(vol.drive_serial_number, 0x1234_ABCD);
    assert_eq!(vol.volume_label().unwrap().as_ascii(), Some("OS"));

    assert_eq!(
        link.environment_path().unwrap().to_string_lossy(),
        "%USERPROFILE%\\notes.txt"
    );
}

#[test]
fn a_written_link_always_ends_with_the_terminal_block() {
    use rclip_shell_link::ShellLinkBuilder;

    let bytes = ShellLinkBuilder::new().name("x").build().unwrap();
    assert_eq!(
        &bytes[bytes.len() - 4..],
        &[0, 0, 0, 0],
        "the terminal block is easy to forget"
    );

    let link = ShellLink::parse(&bytes).unwrap();
    assert_eq!(link.extra_data().count(), 0);
}

#[test]
fn a_non_ascii_local_path_gets_the_unicode_link_info_header() {
    use rclip_shell_link::ShellLinkBuilder;

    let bytes = ShellLinkBuilder::new()
        .local_path("C:\\Users\\Jörg\\notes.txt")
        .build()
        .unwrap();
    let link = ShellLink::parse(&bytes).unwrap();
    let info = link.link_info.unwrap();

    assert_eq!(
        info.header_size, 0x24,
        "the Unicode offset fields only exist once LinkInfoHeaderSize reaches 0x24"
    );
    let path = info.local_base_path().unwrap().unwrap();
    assert!(
        matches!(path, ShellStr::Utf16(_)),
        "the Unicode field is preferred when present"
    );
    assert_eq!(path.to_string_lossy(), "C:\\Users\\Jörg\\notes.txt");
}

#[test]
fn the_builder_appends_a_missing_id_list_terminator() {
    use rclip_idlist::ShellItem;
    use rclip_shell_link::ShellLinkBuilder;

    // A one-item list with no terminator, which is what slicing a PIDL out of a
    // CIDA gives you.
    let unterminated = [
        0x14u8, 0x00, 0x1F, 0x50, 0xE0, 0x4F, 0xD0, 0x20, 0xEA, 0x3A, 0x69, 0x10, 0xA2, 0xD8, 0x08,
        0x00, 0x2B, 0x30, 0x30, 0x9D,
    ];

    let bytes = ShellLinkBuilder::new()
        .target_id_list(&unterminated)
        .build()
        .unwrap();
    let link = ShellLink::parse(&bytes).unwrap();
    let mut items = link.target_id_list.unwrap().items();

    assert!(matches!(
        items.next().unwrap().unwrap().parse(),
        ShellItem::RootFolder(_)
    ));
    assert!(items.next().is_none());
    assert!(
        items.is_terminated(),
        "a LinkTargetIDList without a TerminalID is malformed"
    );
}

#[test]
fn the_builder_refuses_a_string_the_format_cannot_express() {
    use rclip_shell_link::ShellLinkBuilder;

    let long = "a".repeat(261);
    let err = ShellLinkBuilder::new().name(&long).build().unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::TooLarge,
        "MS-SHLLINK 10.0 caps NAME_STRING at 260 characters; truncating would produce a \
         link that silently says something else"
    );

    // COMMAND_LINE_ARGUMENTS is the one field the spec explicitly exempts.
    assert!(ShellLinkBuilder::new().arguments(&long).build().is_ok());
}

#[test]
fn the_builder_refuses_a_path_pair_block_that_would_not_fit() {
    use rclip_shell_link::ShellLinkBuilder;

    let long = "b".repeat(300);
    assert!(
        ShellLinkBuilder::new()
            .environment_path(&long)
            .build()
            .is_err(),
        "the ANSI half of the block is a fixed 260 bytes including its NUL"
    );
}

#[test]
fn a_raw_extra_block_round_trips() {
    use rclip_shell_link::ShellLinkBuilder;

    let bytes = ShellLinkBuilder::new()
        .extra_block(0xA000_0004, &932u32.to_le_bytes())
        .extra_block(0xDEAD_BEEF, b"vendor payload")
        .build()
        .unwrap();

    let link = ShellLink::parse(&bytes).unwrap();
    let blocks: Vec<_> = link.extra_data().map(|b| b.unwrap()).collect();
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        blocks[0],
        ExtraDataBlock::ConsoleFe { code_page: 932 }
    ));
    match blocks[1] {
        ExtraDataBlock::Unknown {
            signature, body, ..
        } => {
            assert_eq!(signature, 0xDEAD_BEEF);
            assert_eq!(body, b"vendor payload");
        }
        ref other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn a_written_link_survives_a_round_trip_through_its_own_bytes() {
    use rclip_shell_link::ShellLinkBuilder;

    let first = ShellLinkBuilder::new()
        .name("Round trip")
        .local_path("D:\\data\\file.bin")
        .volume(DriveType::REMOVABLE, 0xCAFE_BABE, "USB DRIVE")
        .build()
        .unwrap();

    let link = ShellLink::parse(&first).unwrap();
    let rebuilt = ShellLinkBuilder::new()
        .name(&link.string_data.name.unwrap().to_string_lossy())
        .local_path(
            &link
                .link_info
                .as_ref()
                .unwrap()
                .local_base_path()
                .unwrap()
                .unwrap()
                .to_string_lossy(),
        )
        .volume(
            link.link_info
                .as_ref()
                .unwrap()
                .volume_id()
                .unwrap()
                .unwrap()
                .drive_type,
            link.link_info
                .as_ref()
                .unwrap()
                .volume_id()
                .unwrap()
                .unwrap()
                .drive_serial_number,
            &link
                .link_info
                .as_ref()
                .unwrap()
                .volume_id()
                .unwrap()
                .unwrap()
                .volume_label()
                .unwrap()
                .to_string_lossy(),
        )
        .build()
        .unwrap();

    assert_eq!(
        first, rebuilt,
        "reading and writing must be exact inverses here"
    );
}

//! Integration tests for `rclip-bookmark`, driven by `corpus/synthetic/rclip-bookmark`.

use rclip_bookmark::{key, Bookmark, Date, EntryKey, Magic, Value};
use rclip_core::{ErrorKind, MAX_DEPTH};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/rclip-bookmark/");
    std::fs::read(format!("{p}{name}")).expect("fixture")
}

fn err_kind(name: &str) -> ErrorKind {
    let bytes = fixture(name);
    match Bookmark::parse(&bytes) {
        Err(e) => e.kind,
        // A parse that succeeds still has to fail somewhere: the malformed
        // fixtures split into ones the header rejects and ones only a full walk
        // catches, and the test should not care which.
        Ok(bm) => bm.validate().expect_err("malformed fixture must not validate").kind,
    }
}

#[test]
fn minimal_book_has_url_and_filename() {
    let bytes = fixture("url-and-filename.bin");
    let bm = Bookmark::parse(&bytes).expect("well-formed minimal bookmark");

    assert_eq!(bm.magic(), Magic::Book);
    assert_eq!(bm.header_size(), 48, "48-byte prolog is the pre-macOS-26 shape");
    assert_eq!(bm.size(), bytes.len(), "declared size must match the fixture");
    assert_eq!(bm.target_url().unwrap(), Some("file:///Users/example/Documents/report.pdf"));
    assert_eq!(bm.target_filename().unwrap(), Some("report.pdf"));
    assert_eq!(bm.display_name().unwrap(), None, "absent key is None, not an error");
    bm.validate().expect("graph is sound");
}

#[test]
fn toc_entries_are_enumerable() {
    let bytes = fixture("url-and-filename.bin");
    let bm = Bookmark::parse(&bytes).unwrap();

    let tocs: Vec<_> = bm.tocs().map(|t| t.unwrap()).collect();
    assert_eq!(tocs.len(), 1, "one TOC, next-offset zero terminates the chain");
    assert_eq!(tocs[0].id(), 1);
    assert_eq!(tocs[0].len(), 2);

    let keys: Vec<u32> = tocs[0]
        .iter()
        .map(|e| e.unwrap().key().unwrap().as_numeric().unwrap())
        .collect();
    assert_eq!(keys, vec![key::TARGET_URL, key::TARGET_FILENAME]);
}

#[test]
fn path_components_come_back_in_order_without_separators() {
    let bytes = fixture("path-components.bin");
    let bm = Bookmark::parse(&bytes).unwrap();

    let parts: Vec<&str> = bm
        .path_components()
        .unwrap()
        .expect("0x1004 present")
        .map(|c| c.unwrap())
        .collect();
    assert_eq!(parts, vec!["Users", "example", "Documents", "report.pdf"]);
    assert_eq!(bm.volume_name().unwrap(), Some("Macintosh HD"));
    assert_eq!(bm.volume_path().unwrap(), Some("/"));
}

#[test]
fn dates_are_big_endian() {
    let bytes = fixture("date-alis.bin");
    let bm = Bookmark::parse(&bytes).unwrap();
    assert_eq!(bm.magic(), Magic::Alis, "'alis' is as valid a signature as 'book'");

    let d = bm.target_creation_date().unwrap().expect("0x1040 present");
    // 2020-01-01T00:00:00Z. Read little-endian the same eight bytes decode to
    // roughly 1e-311, so this assertion is the whole point of the fixture.
    assert_eq!(d.absolute_seconds(), 599_529_600.0);
    assert_eq!(d.unix_seconds(), 1_577_836_800.0);

    let v = bm.get(key::VOLUME_CREATION_DATE).unwrap().unwrap();
    assert_eq!(v.as_date(), Some(Date::from_absolute_seconds(0.0)));
}

#[test]
fn flags_record_carries_both_words() {
    let bytes = fixture("date-alis.bin");
    let bm = Bookmark::parse(&bytes).unwrap();

    let f = bm.target_flags().unwrap().expect("0x1010 present");
    assert!(f.is_directory(), "bit 0x02 is set in the fixture");
    assert!(!f.is_regular_file());
    assert!(f.has(rclip_bookmark::flags::resource::IS_READABLE));
    assert!(
        f.was_asked_for(rclip_bookmark::flags::resource::IS_DIRECTORY),
        "a clear flag bit only means 'false' when the mask says it was sampled"
    );
}

#[test]
fn bit31_key_resolves_to_a_string() {
    let bytes = fixture("named-key-nested.bin");
    let bm = Bookmark::parse(&bytes).unwrap();

    let toc = bm.tocs().next().unwrap().unwrap();
    let entries: Vec<_> = toc.iter().map(|e| e.unwrap()).collect();
    assert!(!entries[0].has_named_key());
    assert!(entries[1].has_named_key(), "bit 31 set means the key names itself");
    assert_eq!(entries[1].key().unwrap(), EntryKey::Named("com.example.custom"));

    assert!(
        bm.get_named("com.example.custom").unwrap().is_some(),
        "lookup by name must follow the same indirection"
    );
    // A reader that misses bit 31 would look for the raw two-billion key and
    // find nothing, so the numeric lookup must miss too.
    assert!(bm.get(entries[1].raw_key()).unwrap().is_none());
}

#[test]
fn shared_subtrees_are_not_cycles() {
    let bytes = fixture("named-key-nested.bin");
    let bm = Bookmark::parse(&bytes).unwrap();

    let array = bm
        .get_named("com.example.custom")
        .unwrap()
        .unwrap()
        .as_array()
        .expect("outer value is an array");
    assert_eq!(array.len(), 2);

    for element in array.iter() {
        let dict = element.unwrap().as_dict().expect("elements are dictionaries");
        let pairs: Vec<_> = dict.iter().map(|p| p.unwrap()).collect();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0.as_str(), Some("alpha"));
        assert_eq!(pairs[0].1.as_str(), Some("one"));
        assert_eq!(pairs[1].0.as_str(), Some("beta"));
        assert_eq!(pairs[1].1.as_i64(), Some(42));
    }
    // Both elements point at the same dictionary record. Sharing is not a
    // cycle, and a depth limit that counted visits instead of nesting would
    // wrongly reject this.
    bm.validate().expect("shared subtree is legal");
}

// --------------------------------------------------------------- malformed

#[test]
fn cyclic_array_terminates_at_the_depth_limit() {
    let bytes = fixture("cyclic-array.bin");
    let bm = Bookmark::parse(&bytes).expect("the header itself is well-formed");

    // Walking by hand: each hop resolves the same record one level deeper.
    let mut value = bm.get(key::TARGET_PATH).unwrap().unwrap();
    let mut hops = 0u32;
    let kind = loop {
        let array = value.as_array().expect("every hop yields the same array");
        match array.get(0) {
            Ok(v) => {
                value = v;
                hops += 1;
                assert!(hops <= MAX_DEPTH, "descent must stop at MAX_DEPTH, not run forever");
            }
            Err(e) => break e.kind,
        }
    };
    assert_eq!(kind, ErrorKind::DepthLimit, "a self-referential offset is a depth failure");
    assert_eq!(hops, MAX_DEPTH - 1, "the limit bites exactly at MAX_DEPTH");

    // And the same thing through the library's own walk, which is what a caller
    // that does not want to write the loop above would use.
    assert_eq!(bm.validate().unwrap_err().kind, ErrorKind::DepthLimit);
}

#[test]
fn toc_chain_that_loops_is_cut_off() {
    let bytes = fixture("toc-self-loop.bin");
    let bm = Bookmark::parse(&bytes).unwrap();

    // The chain is `while next != 0`, so a self-referential next pointer never
    // recurses and the depth limit cannot help. Only the node budget stops it.
    let mut seen = 0usize;
    let mut kind = None;
    for toc in bm.tocs() {
        match toc {
            Ok(_) => {
                seen += 1;
                assert!(seen < 1000, "TOC chain must not iterate forever");
            }
            Err(e) => {
                kind = Some(e.kind);
                break;
            }
        }
    }
    assert!(seen > 0, "the first TOC is well-formed and must be yielded");
    assert_eq!(kind, Some(ErrorKind::Malformed), "the loop is reported, not ignored");
}

#[test]
fn fanout_bomb_is_stopped_by_the_node_budget() {
    let bytes = fixture("fanout-bomb.bin");
    let bm = Bookmark::parse(&bytes).expect("structurally this is an ordinary bookmark");

    // Eight levels of eight shared references: too shallow for the depth limit
    // and far too small for a size check, but 8^8 nodes to walk naively.
    assert_eq!(
        bm.validate().unwrap_err().kind,
        ErrorKind::TooLarge,
        "breadth blow-up must be caught by the node budget, not by MAX_DEPTH"
    );

    // Lazy access is unaffected: the caller only pays for what it reads.
    let top = bm.get(key::TARGET_PATH).unwrap().unwrap().as_array().unwrap();
    assert_eq!(top.len(), 8);
    assert_eq!(top.get(0).unwrap().as_array().unwrap().len(), 8);
}

#[test]
fn out_of_range_record_offset_is_rejected() {
    let bytes = fixture("offset-past-end.bin");
    let bm = Bookmark::parse(&bytes).unwrap();

    // The sound entry still reads.
    assert_eq!(bm.target_filename().unwrap(), Some("victim.txt"));
    // The unsound one fails at the offset, not by returning truncated bytes.
    assert_eq!(bm.target_url().unwrap_err().kind, ErrorKind::BadOffset);
}

#[test]
fn malformed_fixtures_report_the_right_kind() {
    assert_eq!(err_kind("bad-magic.bin"), ErrorKind::BadMagic);
    assert_eq!(err_kind("truncated-header.bin"), ErrorKind::UnexpectedEof);
    assert_eq!(err_kind("header-size-overruns.bin"), ErrorKind::BadLength);
    assert_eq!(err_kind("offset-past-end.bin"), ErrorKind::BadOffset);
    assert_eq!(err_kind("cyclic-array.bin"), ErrorKind::DepthLimit);
    assert_eq!(err_kind("toc-self-loop.bin"), ErrorKind::Malformed);
    assert_eq!(err_kind("fanout-bomb.bin"), ErrorKind::TooLarge);
}

#[test]
fn empty_and_tiny_inputs_do_not_panic() {
    for len in 0..64usize {
        let buf = vec![0u8; len];
        let _ = Bookmark::parse(&buf);
    }
    // A valid signature followed by nothing usable is the interesting case: the
    // magic check passes and every later field has to hold the line.
    for len in 4..64usize {
        let mut buf = b"book".to_vec();
        buf.resize(len, 0xFF);
        let _ = Bookmark::parse(&buf).map(|bm| bm.validate());
    }
}

#[test]
fn trailing_bytes_after_the_declared_size_are_unreachable() {
    let mut bytes = fixture("url-and-filename.bin");
    let real_len = bytes.len();
    bytes.extend_from_slice(b"SECRET-DATA-THAT-FOLLOWS-THE-BOOKMARK");

    let bm = Bookmark::parse(&bytes).expect("embedded bookmarks carry trailing bytes");
    assert_eq!(bm.size(), real_len, "the declared size wins over the slice length");
    assert_eq!(bm.target_filename().unwrap(), Some("report.pdf"));
}

// ------------------------------------------------------------- real capture

#[test]
fn corefoundation_capture_parses() {
    let bytes = fixture("corefoundation-file.bin");
    let bm = Bookmark::parse(&bytes).expect("real NSURL bookmark data");

    assert_eq!(bm.magic(), Magic::Book);
    assert_eq!(bm.version(), rclip_bookmark::VERSION_10040000);
    assert_eq!(bm.header_size(), 48);
    bm.validate().expect("CoreFoundation's own output must validate");

    let parts: Vec<&str> = bm
        .path_components()
        .unwrap()
        .expect("0x1004 present")
        .map(|c| c.unwrap())
        .collect();
    assert_eq!(parts, vec!["private", "tmp", "rclip-bookmark-target.txt"]);

    assert_eq!(bm.volume_name().unwrap(), Some("Macintosh HD"));
    assert_eq!(bm.volume_path().unwrap(), Some("/"));
    assert_eq!(bm.get(key::VOLUME_IS_ROOT).unwrap().unwrap().as_bool(), Some(true));
    assert_eq!(bm.get(key::VOLUME_URL).unwrap().unwrap().as_str(), Some("file:///"));

    // Volume UUID is stored as a string record even though there is a perfectly
    // good UUID record type; asserting it here keeps that surprise from being
    // re-discovered later.
    let uuid = bm.volume_uuid().unwrap().expect("0x2011 present");
    assert_eq!(uuid.len(), 36, "dashed textual UUID, not sixteen raw bytes");

    // What a real bookmark does NOT have. Both writeups list these keys, and
    // neither is emitted by CoreFoundation for a plain file target.
    assert_eq!(bm.target_url().unwrap(), None);
    assert_eq!(bm.target_filename().unwrap(), None);

    let flags = bm.target_flags().unwrap().expect("0x1010 present");
    assert!(flags.is_regular_file());
    assert!(!flags.is_directory());

    let created = bm.target_creation_date().unwrap().expect("0x1040 present");
    assert!(
        created.unix_seconds() > 1_600_000_000.0,
        "a big-endian read gives a sane recent timestamp; a little-endian one gives ~1e-311"
    );

    let ext = bm.sandbox_extension().unwrap().expect("0xf080 present");
    assert!(
        ext.starts_with(b"cd0e") || ext.contains(&b';'),
        "sandbox extensions are semicolon-delimited tokens, handed back opaque"
    );
}

#[test]
fn cnid_path_matches_path_components() {
    let bytes = fixture("corefoundation-file.bin");
    let bm = Bookmark::parse(&bytes).unwrap();

    let cnids = bm.get(key::TARGET_CNID_PATH).unwrap().unwrap().as_array().unwrap();
    let parts = bm.path_components().unwrap().unwrap();
    assert_eq!(
        cnids.len(),
        parts.len(),
        "0x1005 has one inode per component of 0x1004 — that pairing is what \
         lets macOS resolve a bookmark after the file has been moved"
    );
    for c in cnids.iter() {
        assert!(c.unwrap().as_i64().unwrap() > 0);
    }
}

#[cfg(feature = "alloc")]
#[test]
fn target_path_joins_components() {
    let bytes = fixture("corefoundation-file.bin");
    let bm = Bookmark::parse(&bytes).unwrap();
    assert_eq!(
        bm.target_path().unwrap().as_deref(),
        Some("/private/tmp/rclip-bookmark-target.txt")
    );
}

#[test]
fn every_fixture_matches_its_sidecar() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/rclip-bookmark/");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).expect("corpus dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let sidecar = path.with_extension("json");
        let text = std::fs::read_to_string(&sidecar)
            .unwrap_or_else(|_| panic!("every .bin needs a .json sidecar: {sidecar:?}"));
        let expect_ok = text.contains("\"expect\": \"ok\"");
        let bytes = std::fs::read(&path).unwrap();
        let outcome = Bookmark::parse(&bytes).and_then(|bm| bm.validate());
        assert_eq!(
            outcome.is_ok(),
            expect_ok,
            "{path:?} disagrees with its sidecar's \"expect\" field: {outcome:?}"
        );
        checked += 1;
    }
    assert!(checked >= 11, "expected the whole synthetic corpus, saw {checked}");
}

#[test]
fn unknown_types_survive_as_raw_bytes() {
    // A record with a type nobody has identified must not sink the bookmark:
    // the format is reverse-engineered and new types keep appearing.
    let bytes = fixture("url-and-filename.bin");
    let mut mutated = bytes.clone();
    // The first record starts at byte 52 (48 header + 4 TOC pointer); its type
    // word is the second u32 of the record.
    mutated[56..60].copy_from_slice(&0x0000_1234u32.to_le_bytes());

    let bm = Bookmark::parse(&mutated).unwrap();
    match bm.get(key::TARGET_URL).unwrap().unwrap() {
        Value::Unknown { type_code, data } => {
            assert_eq!(type_code, 0x0000_1234);
            assert_eq!(data.len(), 42);
        }
        other => panic!("unrecognised types must round-trip as raw bytes, got {other:?}"),
    }
    bm.validate().expect("an unknown leaf type is not a structural error");
}

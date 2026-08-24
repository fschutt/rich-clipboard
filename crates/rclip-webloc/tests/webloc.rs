//! Integration tests for `rclip-webloc`, driven by `corpus/synthetic/rclip-webloc`.

use rclip_core::ErrorKind;
use rclip_webloc::{Encoding, Text, Webloc};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-webloc/"
    );
    std::fs::read(format!("{p}{name}")).expect("fixture")
}

/// Decode a `Text` the way a caller without `alloc` has to: one `char` at a
/// time. Also proves the iterator terminates on real input.
fn decode(t: Text<'_>) -> String {
    t.chars().map(|c| c.expect("well-formed text")).collect()
}

#[test]
fn finder_created_xml_webloc() {
    let bytes = fixture("finder-created.bin");
    let loc = Webloc::parse(&bytes).expect("real Finder output");

    assert_eq!(
        loc.encoding(),
        Encoding::Xml,
        "Finder's scripting API writes XML"
    );
    assert_eq!(decode(loc.url()), "https://example.com/rich-clipboard");
    assert_eq!(
        loc.url().as_str(),
        Some("https://example.com/rich-clipboard"),
        "an unescaped value is borrowed straight out of the document"
    );
    assert!(loc.url_name().is_none(), "a plain .webloc has no URLName");
}

#[test]
fn corefoundation_binary_webloc() {
    let bytes = fixture("finder-binary.bin");
    let loc = Webloc::parse(&bytes).expect("CoreFoundation binary output");

    assert_eq!(loc.encoding(), Encoding::Binary);
    // 34 characters, past the 15 that fit in a marker's low nibble, so this
    // only reads correctly if the 0xF extended-count escape is followed.
    assert_eq!(decode(loc.url()), "https://example.com/rich-clipboard");
    assert_eq!(loc.url().as_str().map(str::len), Some(34));
}

#[test]
fn both_encodings_of_the_same_file_agree() {
    let xml = fixture("finder-created.bin");
    let bin = fixture("finder-binary.bin");
    let a = Webloc::parse(&xml).unwrap();
    let b = Webloc::parse(&bin).unwrap();

    assert_ne!(a.encoding(), b.encoding());
    assert_eq!(
        decode(a.url()),
        decode(b.url()),
        "encoding must not change the value"
    );
}

#[test]
fn xml_entities_are_decoded() {
    let bytes = fixture("xml-entities.bin");
    let loc = Webloc::parse(&bytes).unwrap();

    // Every URL with two query parameters has an escaped '&' in it, so this is
    // the ordinary case rather than an exotic one.
    assert_eq!(
        decode(loc.url()),
        "https://example.com/s?q=a&b<c>d&e=\"f\"&gA"
    );
    assert!(
        loc.url().is_encoded(),
        "an escaped value must not pretend its raw bytes are the answer"
    );
    assert_eq!(loc.url().as_str(), None);
}

#[test]
fn inetloc_carries_a_url_name() {
    let bytes = fixture("inetloc-urlname.bin");
    let loc = Webloc::parse(&bytes).unwrap();

    assert_eq!(decode(loc.url()), "https://www.rust-lang.org/");
    assert_eq!(
        decode(loc.url_name().expect("URLName present")),
        "The Rust Programming Language"
    );
}

#[test]
fn binary_utf16_strings_are_big_endian() {
    let bytes = fixture("bplist-utf16.bin");
    let loc = Webloc::parse(&bytes).unwrap();

    // A bplist string is 0x6n UTF-16 as soon as one character is non-ASCII —
    // and big-endian, where every Win32 structure in this workspace is little.
    assert!(matches!(loc.url(), Text::Utf16Be(_)));
    assert_eq!(decode(loc.url()), "https://xn--r8jz45g.example/ページ");
    assert_eq!(decode(loc.url_name().unwrap()), "例え");
}

#[test]
fn eq_str_matches_across_encodings() {
    for name in ["finder-created.bin", "finder-binary.bin"] {
        let bytes = fixture(name);
        let loc = Webloc::parse(&bytes).unwrap();
        assert!(
            loc.url().eq_str("https://example.com/rich-clipboard"),
            "{name}: comparison must work without materialising the string"
        );
        assert!(!loc.url().eq_str("https://example.com/rich-clipboar"));
        assert!(!loc.url().eq_str("https://example.com/rich-clipboardx"));
    }
}

// --------------------------------------------------------------- malformed

#[test]
fn a_renamed_text_file_is_not_a_webloc() {
    let bytes = fixture("not-a-plist.bin");
    assert_eq!(Webloc::detect(&bytes), None);
    assert_eq!(Webloc::parse(&bytes).unwrap_err().kind, ErrorKind::BadMagic);
}

#[test]
fn a_plist_without_a_url_key_is_rejected() {
    let bytes = fixture("xml-no-url-key.bin");
    // Detection succeeds — it really is a plist — and parsing still has to
    // fail, because URL is the entire content of the format.
    assert_eq!(Webloc::detect(&bytes), Some(Encoding::Xml));
    assert_eq!(
        Webloc::parse(&bytes).unwrap_err().kind,
        ErrorKind::Malformed
    );
}

#[test]
fn truncated_binary_plist_is_rejected() {
    // A bplist trailer is positional: it is whatever the last 32 bytes are.
    // Truncating the file therefore does not produce a short read, it produces
    // 32 bytes of object data parsed as a trailer — so the failure is a
    // structural one, not an EOF.
    let bytes = fixture("bplist-truncated.bin");
    assert_eq!(
        Webloc::parse(&bytes).unwrap_err().kind,
        ErrorKind::Malformed
    );

    // Below header-plus-trailer there is not room for both, and that much a
    // length check does catch.
    let short = fixture("bplist-too-short.bin");
    assert_eq!(
        Webloc::parse(&short).unwrap_err().kind,
        ErrorKind::UnexpectedEof
    );
}

#[test]
fn self_referential_dictionary_does_not_hang() {
    let bytes = fixture("bplist-self-referential.bin");
    // The value of URL is a reference back to the root dictionary. This test
    // failing to complete is as much a failure as it returning the wrong kind.
    let err = Webloc::parse(&bytes).unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::Unsupported,
        "a container where a string belongs is a type error before it is a depth error"
    );
}

#[test]
fn object_offset_past_the_end_is_rejected() {
    let bytes = fixture("bplist-offset-past-end.bin");
    assert_eq!(
        Webloc::parse(&bytes).unwrap_err().kind,
        ErrorKind::BadOffset
    );
}

#[test]
fn truncations_of_every_fixture_never_panic() {
    // The cheapest fuzz there is: every prefix of every corpus file.
    for name in [
        "finder-created.bin",
        "finder-binary.bin",
        "bplist-utf16.bin",
        "xml-entities.bin",
        "inetloc-urlname.bin",
    ] {
        let bytes = fixture(name);
        for len in 0..bytes.len() {
            if let Ok(loc) = Webloc::parse(&bytes[..len]) {
                // Whatever came back must still decode without panicking.
                let _ = loc.url().chars().count();
                let _ = loc.url_name().map(|t| t.chars().count());
            }
        }
    }
}

#[test]
fn corrupting_one_byte_never_panics() {
    let bytes = fixture("finder-binary.bin");
    for i in 0..bytes.len() {
        for patch in [0x00u8, 0x0F, 0x7F, 0xD1, 0xFF] {
            let mut m = bytes.clone();
            m[i] = patch;
            if let Ok(loc) = Webloc::parse(&m) {
                let _ = loc.url().chars().count();
            }
        }
    }
}

#[test]
fn every_fixture_matches_its_sidecar() {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-webloc/"
    );
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).expect("corpus dir") {
        let path = entry.unwrap().path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("bin") => {}
            _ => continue,
        }
        let sidecar = path.with_extension("json");
        let text = std::fs::read_to_string(&sidecar)
            .unwrap_or_else(|_| panic!("every fixture needs a .json sidecar: {sidecar:?}"));
        let expect_ok = text.contains("\"expect\": \"ok\"");
        let bytes = std::fs::read(&path).unwrap();
        let outcome = Webloc::parse(&bytes);
        assert_eq!(
            outcome.is_ok(),
            expect_ok,
            "{path:?} disagrees with its sidecar's \"expect\" field: {outcome:?}"
        );
        checked += 1;
    }
    assert!(
        checked >= 10,
        "expected the whole synthetic corpus, saw {checked}"
    );
}

#[cfg(feature = "alloc")]
#[test]
fn to_string_lossy_matches_the_char_iterator() {
    for name in ["xml-entities.bin", "bplist-utf16.bin", "finder-created.bin"] {
        let bytes = fixture(name);
        let loc = Webloc::parse(&bytes).unwrap();
        assert_eq!(loc.url().to_string_lossy(), decode(loc.url()), "{name}");
    }
}

// ---------------------------------------------------------------------------
// The legacy resource-fork form
// ---------------------------------------------------------------------------

fn capture(name: &str) -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/macos/finder/");
    std::fs::read(format!("{p}{name}")).expect("capture")
}

#[test]
fn the_captured_finder_resource_fork_holds_the_same_url_as_its_data_fork() {
    // Same file on disk: `finder-created.bin` is its data fork and
    // `webloc-resource-fork.bin` is its resource fork. Finder writes both, and
    // they have to agree or one of the two readers is wrong.
    let rsrc = capture("webloc-resource-fork.bin");
    let loc = Webloc::parse(&rsrc).expect("a real Finder resource fork");

    assert_eq!(loc.encoding(), Encoding::ResourceFork);
    assert_eq!(decode(loc.url()), "https://example.com/rich-clipboard");
    assert_eq!(
        decode(loc.url()),
        decode(Webloc::parse(&fixture("finder-created.bin")).unwrap().url()),
        "the two forks of one file disagree"
    );
    assert!(
        loc.url_name().is_none(),
        "this form carries no title; Finder puts the name in the filename"
    );
}

#[test]
fn the_captured_fork_lists_the_three_resources_finder_writes() {
    use rclip_webloc::{rsrc, ResourceFork};

    let bytes = capture("webloc-resource-fork.bin");
    let fork = ResourceFork::parse(&bytes).expect("a real Finder resource fork");

    // Exactly what `DeRez` prints for the file this was cut from.
    let types: Vec<_> = fork.types().map(|t| (t.code, t.count)).collect();
    assert_eq!(
        types,
        vec![
            (rsrc::TYPE_DRAG, 1),
            (rsrc::TYPE_TEXT, 1),
            (rsrc::TYPE_URL, 1)
        ],
        "the type list is sorted, and every count is stored minus one"
    );

    let url = fork.first_resource(rsrc::TYPE_URL).unwrap().unwrap();
    assert_eq!(url.id, 256);
    assert_eq!(url.name, None, "Finder names none of them");
    assert!(!url.is_compressed());
    assert_eq!(url.data, b"https://example.com/rich-clipboard");

    // Finder writes the URL twice: once as `url `, once as `TEXT` so that
    // dragging the file somewhere expecting text produces something useful.
    let text = fork.first_resource(rsrc::TYPE_TEXT).unwrap().unwrap();
    assert_eq!(text.data, url.data);

    // And a `drag` resource naming the other two, which is where those four
    // character codes appear a second time.
    let drag = fork.first_resource(rsrc::TYPE_DRAG).unwrap().unwrap();
    assert_eq!(drag.id, 128);
    assert!(drag.data.windows(4).any(|w| w == rsrc::TYPE_URL));
    assert!(drag.data.windows(4).any(|w| w == rsrc::TYPE_TEXT));
}

#[test]
fn resource_names_and_negative_ids_come_back_intact() {
    use rclip_webloc::{rsrc, ResourceFork};

    let bytes = fixture("rsrc-named-resources.bin");
    let fork = ResourceFork::parse(&bytes).expect("Rez output");

    let url = fork.first_resource(rsrc::TYPE_URL).unwrap().unwrap();
    assert_eq!(url.id, 128);
    assert_eq!(url.name, Some(&b"Example destination"[..]));

    let text = fork.first_resource(rsrc::TYPE_TEXT).unwrap().unwrap();
    assert_eq!(
        text.id, -16000,
        "resource IDs are signed; reading this as a u16 gives 49536"
    );
    assert_eq!(text.name, Some(&b"Second name"[..]));

    let loc = Webloc::parse(&bytes).expect("it has a url resource");
    assert_eq!(decode(loc.url()), "https://example.com/legacy");
}

#[test]
fn a_fork_without_a_url_resource_is_not_a_location_file() {
    let bytes = fixture("rsrc-no-url-resource.bin");
    assert_eq!(
        Webloc::parse(&bytes).unwrap_err().kind,
        ErrorKind::Malformed,
        "the same answer a plist without a URL key gets"
    );
}

#[test]
fn a_header_that_does_not_agree_with_the_buffer_is_not_a_resource_fork_at_all() {
    // The distinction the two fixtures exist for: recognising a resource fork
    // *is* checking its header, so a header that lies means "not this format"
    // rather than "this format, broken".
    assert_eq!(
        Webloc::parse(&fixture("rsrc-map-past-end.bin"))
            .unwrap_err()
            .kind,
        ErrorKind::BadMagic
    );
    assert_eq!(
        Webloc::detect(&fixture("rsrc-map-past-end.bin")),
        None,
        "and it is not mistaken for a plist either"
    );
    for name in [
        "rsrc-type-list-past-map.bin",
        "rsrc-data-offset-past-end.bin",
    ] {
        assert_eq!(
            Webloc::detect(&fixture(name)),
            Some(Encoding::ResourceFork),
            "{name}: the header is sound, so the sniff has to accept it"
        );
        assert_eq!(
            Webloc::parse(&fixture(name)).unwrap_err().kind,
            ErrorKind::BadOffset,
            "{name}"
        );
    }
}

#[test]
fn no_plist_is_ever_read_as_a_resource_fork_and_no_fork_as_a_plist() {
    for name in [
        "finder-created.bin",
        "finder-binary.bin",
        "bplist-utf16.bin",
        "xml-entities.bin",
        "inetloc-urlname.bin",
        "not-a-plist.bin",
    ] {
        assert_ne!(
            Webloc::detect(&fixture(name)),
            Some(Encoding::ResourceFork),
            "{name} was sniffed as a resource fork"
        );
    }
    assert_eq!(
        Webloc::detect(&capture("webloc-resource-fork.bin")),
        Some(Encoding::ResourceFork)
    );
}

#[test]
fn truncations_and_corruptions_of_a_resource_fork_never_panic() {
    use rclip_webloc::ResourceFork;

    for name in ["rsrc-named-resources.bin", "rsrc-no-url-resource.bin"] {
        let bytes = fixture(name);
        for len in 0..bytes.len() {
            let _ = Webloc::parse(&bytes[..len]);
            if let Ok(fork) = ResourceFork::parse(&bytes[..len]) {
                for t in fork.types() {
                    for r in t.resources() {
                        let _ = r.map(|r| r.data.len());
                    }
                }
            }
        }
    }

    let bytes = capture("webloc-resource-fork.bin");
    for i in 0..bytes.len() {
        for patch in [0x00u8, 0x0F, 0x7F, 0xD1, 0xFF] {
            let mut m = bytes.clone();
            m[i] = patch;
            if let Ok(loc) = Webloc::parse(&m) {
                let _ = loc.url().chars().count();
            }
            if let Ok(fork) = ResourceFork::parse(&m) {
                for t in fork.types() {
                    for r in t.resources() {
                        let _ = r.map(|r| r.data.len());
                    }
                }
            }
        }
    }
}

#[test]
fn finder_info_tells_an_internet_location_file_from_anything_else() {
    use rclip_webloc::is_internet_location_finder_info;

    // Captured with `xattr -p com.apple.FinderInfo` on files Finder wrote for
    // one URL of each scheme, on macOS 15.5.
    for observed in [
        b"ilhtMACS", // .webloc
        b"ilftMACS", // .ftploc
        b"ilmaMACS", // .mailloc
        b"ilfiMACS", // .fileloc
        b"ilafMACS", // .afploc
        b"ilnwMACS", // .nntploc
    ] {
        let mut info = [0u8; 32];
        info[..8].copy_from_slice(observed);
        assert!(
            is_internet_location_finder_info(&info),
            "{:?}",
            core::str::from_utf8(observed)
        );
    }

    // A plain text clipping, and a type that is `il` with somebody else's
    // creator code.
    for other in [&b"clipTEXT"[..], b"ilhtXXXX", b"", b"ilht"] {
        let mut info = [0u8; 32];
        info[..other.len()].copy_from_slice(other);
        assert!(!is_internet_location_finder_info(&info[..other.len()]));
    }
}

#[test]
fn a_dict_fanout_bomb_is_stopped_by_the_node_budget_not_by_depth() {
    use rclip_core::ErrorKind;
    use rclip_webloc::bplist::{BinaryPlist, Object};

    let bomb = fixture("bplist-dict-fanout-bomb.bin");
    assert!(bomb.len() < 300, "the whole point is that it is tiny");

    let p = BinaryPlist::parse(&bomb).expect("structurally a valid bplist");
    let mut budget = p.budget();

    // Walk it the way a consumer would. Without the budget this resolves
    // 9^9 objects — measured at 40 million visits and 5.8 seconds — while
    // never exceeding depth 9, so MAX_DEPTH never fires.
    fn walk(
        p: &BinaryPlist<'_>,
        index: usize,
        depth: u32,
        budget: &mut usize,
    ) -> Option<ErrorKind> {
        match p.object(index, depth, budget) {
            Err(e) => Some(e.kind),
            Ok(Object::Dict { values, count, .. }) => {
                for i in 0..count {
                    let next = p.reference(values, i).ok()?;
                    if let Some(kind) = walk(p, next, depth + 1, budget) {
                        return Some(kind);
                    }
                }
                None
            }
            Ok(_) => None,
        }
    }

    let stopped = walk(&p, p.top_object(), 0, &mut budget);
    assert_eq!(
        stopped,
        Some(ErrorKind::TooLarge),
        "the walk must run out of budget; depth never reaches MAX_DEPTH here"
    );

    // And the crate's own entry point was never vulnerable: it looks up one
    // key at the root rather than walking.
    assert!(rclip_webloc::Webloc::parse(&bomb).is_err());
}

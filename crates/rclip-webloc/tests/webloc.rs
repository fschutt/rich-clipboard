//! Integration tests for `rclip-webloc`, driven by `corpus/synthetic/rclip-webloc`.

use rclip_core::ErrorKind;
use rclip_webloc::{Encoding, Text, Webloc};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/rclip-webloc/");
    std::fs::read(format!("{p}{name}")).expect("fixture")
}

/// Decode a `Text` the way a caller without `alloc` has to: one `char` at a
/// time. Also proves the iterator terminates on real input.
fn decode(t: Text<'_>) -> String {
    t.chars().map(|c| c.expect("well-formed text")).collect()
}

#[test]
fn finder_created_xml_webloc() {
    let bytes = fixture("finder-created.webloc");
    let loc = Webloc::parse(&bytes).expect("real Finder output");

    assert_eq!(loc.encoding(), Encoding::Xml, "Finder's scripting API writes XML");
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
    let bytes = fixture("finder-binary.webloc");
    let loc = Webloc::parse(&bytes).expect("CoreFoundation binary output");

    assert_eq!(loc.encoding(), Encoding::Binary);
    // 34 characters, past the 15 that fit in a marker's low nibble, so this
    // only reads correctly if the 0xF extended-count escape is followed.
    assert_eq!(decode(loc.url()), "https://example.com/rich-clipboard");
    assert_eq!(loc.url().as_str().map(str::len), Some(34));
}

#[test]
fn both_encodings_of_the_same_file_agree() {
    let xml = fixture("finder-created.webloc");
    let bin = fixture("finder-binary.webloc");
    let a = Webloc::parse(&xml).unwrap();
    let b = Webloc::parse(&bin).unwrap();

    assert_ne!(a.encoding(), b.encoding());
    assert_eq!(decode(a.url()), decode(b.url()), "encoding must not change the value");
}

#[test]
fn xml_entities_are_decoded() {
    let bytes = fixture("xml-entities.webloc");
    let loc = Webloc::parse(&bytes).unwrap();

    // Every URL with two query parameters has an escaped '&' in it, so this is
    // the ordinary case rather than an exotic one.
    assert_eq!(decode(loc.url()), "https://example.com/s?q=a&b<c>d&e=\"f\"&gA");
    assert!(
        loc.url().is_encoded(),
        "an escaped value must not pretend its raw bytes are the answer"
    );
    assert_eq!(loc.url().as_str(), None);
}

#[test]
fn inetloc_carries_a_url_name() {
    let bytes = fixture("inetloc-urlname.inetloc");
    let loc = Webloc::parse(&bytes).unwrap();

    assert_eq!(decode(loc.url()), "https://www.rust-lang.org/");
    assert_eq!(
        decode(loc.url_name().expect("URLName present")),
        "The Rust Programming Language"
    );
}

#[test]
fn binary_utf16_strings_are_big_endian() {
    let bytes = fixture("bplist-utf16.webloc");
    let loc = Webloc::parse(&bytes).unwrap();

    // A bplist string is 0x6n UTF-16 as soon as one character is non-ASCII —
    // and big-endian, where every Win32 structure in this workspace is little.
    assert!(matches!(loc.url(), Text::Utf16Be(_)));
    assert_eq!(decode(loc.url()), "https://xn--r8jz45g.example/ページ");
    assert_eq!(decode(loc.url_name().unwrap()), "例え");
}

#[test]
fn eq_str_matches_across_encodings() {
    for name in ["finder-created.webloc", "finder-binary.webloc"] {
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
    let bytes = fixture("not-a-plist.webloc");
    assert_eq!(Webloc::detect(&bytes), None);
    assert_eq!(Webloc::parse(&bytes).unwrap_err().kind, ErrorKind::BadMagic);
}

#[test]
fn a_plist_without_a_url_key_is_rejected() {
    let bytes = fixture("xml-no-url-key.webloc");
    // Detection succeeds — it really is a plist — and parsing still has to
    // fail, because URL is the entire content of the format.
    assert_eq!(Webloc::detect(&bytes), Some(Encoding::Xml));
    assert_eq!(Webloc::parse(&bytes).unwrap_err().kind, ErrorKind::Malformed);
}

#[test]
fn truncated_binary_plist_is_rejected() {
    // A bplist trailer is positional: it is whatever the last 32 bytes are.
    // Truncating the file therefore does not produce a short read, it produces
    // 32 bytes of object data parsed as a trailer — so the failure is a
    // structural one, not an EOF.
    let bytes = fixture("bplist-truncated.webloc");
    assert_eq!(Webloc::parse(&bytes).unwrap_err().kind, ErrorKind::Malformed);

    // Below header-plus-trailer there is not room for both, and that much a
    // length check does catch.
    let short = fixture("bplist-too-short.webloc");
    assert_eq!(Webloc::parse(&short).unwrap_err().kind, ErrorKind::UnexpectedEof);
}

#[test]
fn self_referential_dictionary_does_not_hang() {
    let bytes = fixture("bplist-self-referential.webloc");
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
    let bytes = fixture("bplist-offset-past-end.webloc");
    assert_eq!(Webloc::parse(&bytes).unwrap_err().kind, ErrorKind::BadOffset);
}

#[test]
fn truncations_of_every_fixture_never_panic() {
    // The cheapest fuzz there is: every prefix of every corpus file.
    for name in [
        "finder-created.webloc",
        "finder-binary.webloc",
        "bplist-utf16.webloc",
        "xml-entities.webloc",
        "inetloc-urlname.inetloc",
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
    let bytes = fixture("finder-binary.webloc");
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
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/rclip-webloc/");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).expect("corpus dir") {
        let path = entry.unwrap().path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("webloc" | "inetloc") => {}
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
    assert!(checked >= 10, "expected the whole synthetic corpus, saw {checked}");
}

#[cfg(feature = "alloc")]
#[test]
fn to_string_lossy_matches_the_char_iterator() {
    for name in ["xml-entities.webloc", "bplist-utf16.webloc", "finder-created.webloc"] {
        let bytes = fixture(name);
        let loc = Webloc::parse(&bytes).unwrap();
        assert_eq!(loc.url().to_string_lossy(), decode(loc.url()), "{name}");
    }
}

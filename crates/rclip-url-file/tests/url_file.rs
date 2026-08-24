//! Integration tests for `.url` parsing, driven by `corpus/synthetic`.
//!
//! The format has no specification, so most of these assert against a
//! *reimplementation* — Wine's `intshcut.c` — or against the unofficial guide.
//! Each test names which.

use rclip_core::ErrorKind;
use rclip_url_file::{parse, HotKey, ShortcutTarget, ShowCommand};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-url-file/"
    );
    std::fs::read(format!("{p}{name}")).expect("fixture")
}

#[test]
fn reads_every_documented_key() {
    let bytes = fixture("full.bin");
    let f = parse(&bytes).expect("well-formed");

    assert_eq!(f.url(), Some("http://www.someaddress.com/"));
    assert_eq!(f.working_directory(), Some(r"C:\WINDOWS\"));
    assert_eq!(f.icon_file(), Some(r"C:\WINDOWS\SYSTEM\url.dll"));
    assert_eq!(f.icon_index().unwrap().unwrap(), 1);
    assert_eq!(
        f.show_command().unwrap().unwrap(),
        ShowCommand::MIN_NO_ACTIVE
    );
    assert_eq!(f.id_list(), Some(""));
}

#[test]
fn hotkey_splits_into_virtual_key_and_modifiers() {
    let bytes = fixture("full.bin");
    let f = parse(&bytes).unwrap();
    let hk = f.hotkey().unwrap().unwrap();

    // The cyanwerks table lists 1601 as Ctrl+Alt+A; 1601 == 0x0641.
    assert_eq!(hk.key, b'A', "low byte is the virtual-key code");
    assert!(hk.has(HotKey::CONTROL), "0x06 has HOTKEYF_CONTROL");
    assert!(hk.has(HotKey::ALT), "0x06 has HOTKEYF_ALT");
    assert!(!hk.has(HotKey::SHIFT), "0x06 does not have HOTKEYF_SHIFT");
    assert_eq!(hk.to_word(), 1601, "round-trips back to the stored word");
}

#[test]
fn modified_is_a_little_endian_filetime_plus_a_trailing_byte() {
    let bytes = fixture("full.bin");
    let f = parse(&bytes).unwrap();
    let m = f.modified().unwrap().unwrap();

    // 20 F0 6B A0 6D 07 BD 01, little-endian.
    assert_eq!(
        m.filetime, 0x01BD_076D_A06B_F020,
        "the hex reads backwards relative to a FILETIME printed high-word first"
    );
    assert_eq!(
        m.trailing, "4D",
        "the ninth byte is handed back, not validated"
    );
}

#[test]
fn minimal_file_with_lf_terminators_parses() {
    let bytes = fixture("minimal-lf.bin");
    let f = parse(&bytes).unwrap();
    assert_eq!(f.require_url().unwrap(), "https://example.com/");
    assert_eq!(
        f.target(),
        Some(ShortcutTarget::Url("https://example.com/"))
    );
}

#[test]
fn ansi_and_wide_sections_are_returned_verbatim() {
    let bytes = fixture("ansi-wide-sections.bin");
    let f = parse(&bytes).unwrap();

    assert_eq!(f.url(), Some("https://example.com/caf%C3%A9"));
    assert_eq!(f.url_ansi(), Some("https://example.com/caf%C3%A9"));
    assert_eq!(f.url_wide(), Some("https://example.com/caf%C3%A9"));

    let names: Vec<_> = f.sections().map(|s| s.name()).collect();
    assert_eq!(
        names,
        vec![
            "{000214A0-0000-0000-C000-000000000046}",
            "InternetShortcut",
            "InternetShortcut.A",
            "InternetShortcut.W",
        ],
        "the property-set section must not swallow the ones after it"
    );
}

#[test]
fn section_and_key_names_are_case_insensitive() {
    // Wine writes ICONINDEX= and reads iconindex; that only works because
    // GetPrivateProfileString folds case.
    let bytes = fixture("case-and-quotes.bin");
    let f = parse(&bytes).unwrap();

    assert!(
        f.internet_shortcut().is_some(),
        "[internetshortcut] must match"
    );
    assert_eq!(
        f.url(),
        Some("https://example.com/x"),
        "quotes are stripped, spaces trimmed"
    );
    assert_eq!(f.icon_index().unwrap().unwrap(), 2, "0x2 is hex");
}

#[test]
fn semicolon_starts_a_comment_and_hash_does_not() {
    let bytes = fixture("case-and-quotes.bin");
    let f = parse(&bytes).unwrap();
    let keys: Vec<_> = f
        .internet_shortcut()
        .unwrap()
        .entries()
        .map(|e| e.key)
        .collect();
    assert_eq!(
        keys,
        vec!["url", "ICONINDEX"],
        "the ';' line is not an entry"
    );
}

#[test]
fn a_byte_order_mark_does_not_hide_the_first_section() {
    let bytes = fixture("bom.bin");
    let f = parse(&bytes).unwrap();
    assert_eq!(f.url(), Some("https://example.net/"));
}

#[test]
fn offsets_are_relative_to_the_original_buffer_not_past_the_bom() {
    let bytes = fixture("bom.bin");
    let f = parse(&bytes).unwrap();
    let entry = f.internet_shortcut().unwrap().entries().next().unwrap();
    // BOM (3) + "[InternetShortcut]\r\n" (20).
    assert_eq!(
        entry.offset, 23,
        "an offset must be findable in a hex dump of the file"
    );
}

#[test]
fn a_file_without_a_url_is_malformed_only_when_the_url_is_demanded() {
    let bytes = fixture("no-url.bin");
    let f = parse(&bytes).expect("structurally fine, so parse must succeed");
    assert_eq!(f.url(), None);
    assert_eq!(
        f.icon_file(),
        Some(r"C:\x.ico"),
        "the rest of the file is still readable"
    );
    assert_eq!(f.require_url().unwrap_err().kind, ErrorKind::Malformed);
}

#[test]
fn an_unterminated_section_header_is_rejected() {
    let bytes = fixture("unterminated-section.bin");
    let err = parse(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
    assert_eq!(err.offset, 0, "the offset points at the bad header");
}

#[test]
fn a_key_before_the_first_section_is_rejected() {
    let bytes = fixture("key-before-section.bin");
    let err = parse(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
    assert_eq!(err.offset, 0, "the orphan line is the first one");
}

#[test]
fn non_utf8_is_reported_with_the_offset_of_the_bad_byte() {
    let err = parse(b"[InternetShortcut]\r\nURL=https://x/\xff\r\n").unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidUtf8);
    assert_eq!(err.offset, 34);
}

#[test]
fn a_drive_letter_is_a_path_not_a_one_letter_url_scheme() {
    // `C:` satisfies RFC 3986's scheme production, so classification order is
    // the whole correctness of this function.
    for (input, want) in [
        (
            r"C:\Users\me\x.txt",
            ShortcutTarget::Path(r"C:\Users\me\x.txt"),
        ),
        (
            r"\\server\share\x",
            ShortcutTarget::Path(r"\\server\share\x"),
        ),
        ("/home/me/x", ShortcutTarget::Path("/home/me/x")),
        (
            "https://example.com/",
            ShortcutTarget::Url("https://example.com/"),
        ),
        ("file:///C:/x", ShortcutTarget::Url("file:///C:/x")),
        (
            "mailto:a@b.example",
            ShortcutTarget::Url("mailto:a@b.example"),
        ),
        ("readme.txt", ShortcutTarget::Unresolved("readme.txt")),
        ("", ShortcutTarget::Unresolved("")),
    ] {
        assert_eq!(
            ShortcutTarget::classify(input),
            want,
            "classifying {input:?}"
        );
    }
}

#[test]
fn a_malformed_modified_value_reports_which_way_it_is_wrong() {
    let short = parse(b"[InternetShortcut]\r\nURL=x:y\r\nModified=DEAD\r\n").unwrap();
    assert_eq!(
        short.modified().unwrap().unwrap_err().kind,
        ErrorKind::BadLength,
        "fewer than sixteen hex digits is not a FILETIME under any reading"
    );

    let bad = parse(b"[InternetShortcut]\r\nURL=x:y\r\nModified=ZZZZZZZZZZZZZZZZ\r\n").unwrap();
    assert_eq!(
        bad.modified().unwrap().unwrap_err().kind,
        ErrorKind::Malformed
    );
}

#[test]
fn an_absent_optional_key_is_none_and_not_an_error() {
    let bytes = fixture("minimal-lf.bin");
    let f = parse(&bytes).unwrap();
    assert!(f.hotkey().is_none());
    assert!(f.icon_index().is_none());
    assert!(f.show_command().is_none());
    assert!(f.modified().is_none());
}

// ---------------------------------------------------------------- sidecars

/// Read `"expect"` out of a sidecar without a JSON dependency. The sidecars are
/// generated to a fixed shape, and a dev-dependency on `serde_json` to read one
/// field would be the largest dependency in the crate.
fn expect_of(json: &str) -> &str {
    let at = json
        .find("\"expect\"")
        .expect("sidecar has an expect field");
    let rest = &json[at + "\"expect\"".len()..];
    let open = rest.find('"').expect("a value follows");
    let tail = &rest[open + 1..];
    &tail[..tail.find('"').expect("the value is terminated")]
}

/// Every fixture is covered, and every sidecar tells the truth.
///
/// The point of this sweep is that a `.json` claiming `"expect": "ok"` cannot
/// quietly stop being true, and that a fixture cannot be added without a test
/// deciding what it means.
#[test]
fn every_fixture_matches_its_sidecar() {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-url-file"
    );
    let mut seen = 0usize;
    for entry in std::fs::read_dir(dir).expect("corpus directory") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        seen += 1;
        let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
        let sidecar = std::fs::read_to_string(path.with_extension("json"))
            .unwrap_or_else(|_| panic!("{stem}.bin has no .json sidecar"));
        let bytes = std::fs::read(&path).unwrap();

        match expect_of(&sidecar) {
            "ok" => {
                let f =
                    parse(&bytes).unwrap_or_else(|e| panic!("{stem} claims ok but failed: {e}"));
                assert!(
                    f.sections().next().is_some(),
                    "{stem}: at least one section"
                );
            }
            "error" => {
                let failed = match stem.as_str() {
                    // parse() is structural; a missing URL is a semantic
                    // failure that only require_url() reports.
                    "no-url" => parse(&bytes).map_or(true, |f| f.require_url().is_err()),
                    _ => parse(&bytes).is_err(),
                };
                assert!(failed, "{stem} claims error but parsed cleanly");
            }
            other => panic!("{stem}: expect must be \"ok\" or \"error\", not {other:?}"),
        }
    }
    assert_eq!(
        seen, 8,
        "a new fixture needs a test that says what it means"
    );
}

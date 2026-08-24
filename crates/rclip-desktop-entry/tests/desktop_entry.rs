//! Integration tests for `.desktop` parsing, driven by `corpus/synthetic`.
//!
//! Weighted towards the three things that make this format hard — escapes,
//! `;`-separated lists, and the locale ladder — because those are where every
//! existing Rust implementation is wrong, and towards `Exec=`, because that is
//! where getting it wrong is dangerous.

use rclip_core::ErrorKind;
use rclip_desktop_entry::{
    parse, EntryType, ExecPiece, FieldCode, Locale, ShortcutTarget, Value,
};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/rclip-desktop-entry/");
    std::fs::read(format!("{p}{name}")).expect("fixture")
}

fn collect(v: Value<'_>) -> String {
    v.chars().map(|c| c.expect("decodes")).collect()
}

// ---------------------------------------------------------------- structure

#[test]
fn a_type_link_entry_is_a_shortcut() {
    let bytes = fixture("link-simple.bin");
    let f = parse(&bytes).unwrap();
    assert_eq!(f.entry_type(), Some(EntryType::Link));
    assert_eq!(f.target(), Some(ShortcutTarget::Url("https://example.com/")));
    assert_eq!(collect(f.url().unwrap()), "https://example.com/");
}

#[test]
fn a_type_application_entry_has_no_shortcut_target() {
    // An Application points at a program, not at a document. Conflating the
    // two is how "open this shortcut" turns into "run this command".
    let bytes = fixture("localized-names.bin");
    let f = parse(&bytes).unwrap();
    assert_eq!(f.entry_type(), Some(EntryType::Application));
    assert_eq!(f.target(), None);
}

#[test]
fn an_unknown_type_is_reported_rather_than_rejected() {
    let src = b"[Desktop Entry]\nType=ServiceType\nName=x\n";
    let f = parse(src).unwrap();
    assert_eq!(f.entry_type(), Some(EntryType::Other("ServiceType")));
}

#[test]
fn a_bom_and_crlf_terminators_do_not_hide_the_group() {
    let bytes = fixture("crlf-and-bom.bin");
    let f = parse(&bytes).unwrap();
    assert!(f.desktop_entry().is_some(), "the group name must not keep a trailing CR");
    assert_eq!(f.target(), Some(ShortcutTarget::Url("https://example.org/")));
}

// ------------------------------------------------------------------ escapes

#[test]
fn the_escape_table_from_section_4_is_complete() {
    let f = parse(b"[Desktop Entry]\nType=Link\nURL=x:y\nComment=a\\sb\\nc\\td\\re\\\\f\n")
        .unwrap();
    assert_eq!(
        collect(f.desktop_entry().unwrap().value("Comment").unwrap()),
        "a b\nc\td\re\\f",
        r"\s is a space - the escape existing crates most often skip"
    );
}

#[test]
fn escapes_decode_in_a_real_file() {
    let bytes = fixture("escaped-list.bin");
    let g = parse(&bytes).unwrap().desktop_entry().unwrap();
    assert_eq!(collect(g.value("Name").unwrap()), "Escaped Space");
    assert_eq!(collect(g.value("Comment").unwrap()), "Line one\nline two\ttabbed");
}

#[test]
fn an_unknown_escape_is_an_error_not_a_guess() {
    let f = parse(b"[Desktop Entry]\nType=Link\nURL=x:y\nComment=bad\\qescape\n").unwrap();
    let err = f
        .desktop_entry()
        .unwrap()
        .value("Comment")
        .unwrap()
        .chars()
        .find_map(Result::err)
        .expect("must report an error");
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn a_dangling_backslash_does_not_eat_the_next_key() {
    // The regression that matters: `freedesktop_entry_parser` treats a
    // backslash anywhere as a line continuation, so `Name=Trailing\` absorbs
    // the following `Exec=` line and the Exec key disappears from the entry.
    let bytes = fixture("unterminated-escape.bin");
    let f = parse(&bytes).expect("structurally valid, so parse must succeed");
    let g = f.desktop_entry().unwrap();

    let err = g.value("Name").unwrap().chars().find_map(Result::err).expect("must fail");
    assert_eq!(err.kind, ErrorKind::UnexpectedEof, "the escape ran off the end of the value");

    assert_eq!(
        g.value("Exec").unwrap().raw(),
        "/usr/bin/evil --pwn",
        "the following key must still be its own entry"
    );
}

#[test]
fn a_boolean_is_only_true_or_false() {
    let f = parse(b"[Desktop Entry]\nType=Application\nTerminal=true\nNoDisplay=1\n").unwrap();
    let g = f.desktop_entry().unwrap();
    assert!(g.boolean("Terminal").unwrap().unwrap());
    assert_eq!(
        g.boolean("NoDisplay").unwrap().unwrap_err().kind,
        ErrorKind::Malformed,
        "0/1 was the pre-1.0 spelling and is listed under Deprecated Items"
    );
}

// -------------------------------------------------------------------- lists

#[test]
fn an_escaped_semicolon_does_not_split_the_list() {
    let bytes = fixture("escaped-list.bin");
    let g = parse(&bytes).unwrap().desktop_entry().unwrap();
    let cats: Vec<String> = g.list("Categories").unwrap().map(collect).collect();
    assert_eq!(
        cats,
        vec!["Network", "Web;Browser", "Utility"],
        r"splitting must happen before unescaping, or \; becomes a separator"
    );
}

#[test]
fn a_trailing_semicolon_terminates_rather_than_adding_an_empty_item() {
    let cases: &[(&str, &[&str])] = &[
        ("a;b;", &["a", "b"]),
        ("a;b", &["a", "b"]),
        ("a;b;;", &["a", "b", ""]),
        ("", &[]),
        (";", &[""]),
    ];
    for (raw, want) in cases {
        let src = format!("[Desktop Entry]\nType=Application\nCategories={raw}\n");
        let f = parse(src.as_bytes()).unwrap();
        let got: Vec<String> =
            f.desktop_entry().unwrap().list("Categories").unwrap().map(collect).collect();
        assert_eq!(&got, want, "splitting {raw:?}");
    }
}

#[test]
fn a_doubled_backslash_before_a_semicolon_still_separates() {
    // `\;` is a literal backslash followed by a real separator.
    let f = parse(b"[Desktop Entry]\nType=Application\nCategories=a\\\\;b;\n").unwrap();
    let got: Vec<String> =
        f.desktop_entry().unwrap().list("Categories").unwrap().map(collect).collect();
    assert_eq!(got, vec!["a\\", "b"]);
}

// ------------------------------------------------------------------ locales

#[test]
fn the_specs_own_locale_example_resolves_the_way_it_says() {
    let bytes = fixture("localized-names.bin");
    let f = parse(&bytes).unwrap();

    // Section 5: "if the current value of the LC_MESSAGES category is
    // sr_YU@Latn ... then the value of the Name keyed by sr_YU is used."
    let l = Locale::parse("sr_YU@Latn").unwrap();
    assert_eq!(collect(f.name(Some(&l)).unwrap()), "Foo sr_YU");
}

#[test]
fn every_rung_of_the_ladder_is_tried() {
    let bytes = fixture("localized-names.bin");
    let f = parse(&bytes).unwrap();
    let cases = [
        ("sr_YU@Latn", "Foo sr_YU"),
        ("sr_YU", "Foo sr_YU"),
        ("sr@Latn", "Foo sr Latn"),
        ("sr", "Foo sr"),
        // No `sr_RS` key, so it falls to `sr`.
        ("sr_RS", "Foo sr"),
        // Nothing German at all: the unpostfixed key.
        ("de_DE", "Foo"),
    ];
    for (locale, want) in cases {
        let l = Locale::parse(locale).unwrap();
        assert_eq!(collect(f.name(Some(&l)).unwrap()), want, "locale {locale}");
    }
}

#[test]
fn a_request_without_a_modifier_never_matches_a_key_with_one() {
    // Section 5: "If LC_MESSAGES does not have a MODIFIER field, then no key
    // with a modifier will be matched."
    let f = parse(b"[Desktop Entry]\nType=Application\nName=Default\nName[sr@Latn]=Latin\n")
        .unwrap();
    let l = Locale::parse("sr").unwrap();
    assert_eq!(collect(f.name(Some(&l)).unwrap()), "Default");
}

#[test]
fn the_encoding_is_ignored_on_both_sides() {
    let bytes = fixture("localized-names.bin");
    let f = parse(&bytes).unwrap();
    let l = Locale::parse("sr_YU.UTF-8@Latn").unwrap();
    assert_eq!(l.country(), Some("YU"), "the codeset must not end up in the country");
    assert_eq!(l.modifier(), Some("Latn"));
    assert_eq!(collect(f.name(Some(&l)).unwrap()), "Foo sr_YU");
}

#[test]
fn no_locale_means_the_unpostfixed_value_not_an_arbitrary_translation() {
    let bytes = fixture("localized-names.bin");
    let f = parse(&bytes).unwrap();
    assert_eq!(collect(f.name(None).unwrap()), "Foo");
}

#[test]
fn candidates_are_produced_in_the_specs_order() {
    let l = Locale::parse("sr_YU@Latn").unwrap();
    let got: Vec<_> = l
        .candidates()
        .map(|c| (c.lang(), c.country(), c.modifier()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("sr", Some("YU"), Some("Latn")),
            ("sr", Some("YU"), None),
            ("sr", None, Some("Latn")),
            ("sr", None, None),
        ]
    );
}

// --------------------------------------------------------------------- exec

#[test]
fn a_quoted_program_path_containing_a_space_is_one_argument() {
    let bytes = fixture("exec-field-codes.bin");
    let f = parse(&bytes).unwrap();
    let cmd = f.exec().unwrap();

    let args: Vec<_> = cmd.args().map(|a| a.expect("well-formed")).collect();
    assert_eq!(args.len(), 3);
    assert_eq!(args[0].raw(), "/opt/my app/bin/view");
    assert!(args[0].quoted());
    assert_eq!(args[1].raw(), "--flag=1");
    assert_eq!(args[2].as_field(), Some(FieldCode::UrlList));
    cmd.validate().expect("this command line obeys section 7");
}

#[test]
fn the_two_escape_layers_run_in_the_specs_order() {
    // Section 7's own examples: four backslashes are one literal backslash in
    // a quoted argument, and `\\$` is a literal dollar sign.
    let bytes = fixture("exec-backslashes.bin");
    let f = parse(&bytes).unwrap();
    let args: Vec<_> = f.exec().unwrap().args().map(|a| a.unwrap()).collect();

    let decode = |a: &rclip_desktop_entry::ExecArg<'_>| -> String {
        a.pieces()
            .map(|p| match p.expect("decodes") {
                ExecPiece::Char(c) => c,
                ExecPiece::Field(_) => panic!("no field codes here"),
                other => panic!("unexpected piece {other:?}"),
            })
            .collect()
    };
    assert_eq!(decode(&args[0]), "/bin/prog");
    assert_eq!(decode(&args[1]), r"a\b", "four backslashes collapse to one");
    assert_eq!(decode(&args[2]), "cost $5", r"\\$ is a literal dollar sign");
}

#[test]
fn an_escaped_space_separates_arguments() {
    // `\s` decodes to a space at the value layer, and the quoting layer then
    // treats that space as a separator — which is why section 7 says an
    // argument containing a space "must be quoted".
    let f = parse(b"[Desktop Entry]\nType=Application\nExec=foo\\sbar\n").unwrap();
    let args: Vec<_> =
        f.exec().unwrap().args().map(|a| a.unwrap().raw().to_string()).collect();
    assert_eq!(args, vec!["foo", "bar"]);
}

#[test]
fn every_field_code_in_section_7_is_recognized() {
    let f = parse(b"[Desktop Entry]\nType=Application\nExec=p %f %u %i %c %k %d %v\n").unwrap();
    let got: Vec<_> = f
        .exec()
        .unwrap()
        .args()
        .skip(1)
        .map(|a| a.unwrap().as_field().expect("a field code on its own"))
        .collect();
    assert_eq!(
        got,
        vec![
            FieldCode::SingleFile,
            FieldCode::SingleUrl,
            FieldCode::Icon,
            FieldCode::TranslatedName,
            FieldCode::DesktopFileLocation,
            FieldCode::Deprecated('d'),
            FieldCode::Deprecated('v'),
        ]
    );
}

#[test]
fn a_double_percent_is_a_literal_percent() {
    let f = parse(b"[Desktop Entry]\nType=Application\nExec=p 100%%\n").unwrap();
    let arg = f.exec().unwrap().args().nth(1).unwrap().unwrap();
    let s: String = arg
        .pieces()
        .map(|p| match p.unwrap() {
            ExecPiece::Char(c) => c,
            ExecPiece::Field(_) => panic!("no field codes here"),
            other => panic!("unexpected piece {other:?}"),
        })
        .collect();
    assert_eq!(s, "100%");
}

#[test]
fn an_unlisted_field_code_is_rejected() {
    // Section 7: "Command lines that contain a field code that is not listed
    // in this specification are invalid and must not be processed."
    let f = parse(b"[Desktop Entry]\nType=Application\nExec=p %z\n").unwrap();
    let arg = f.exec().unwrap().args().nth(1).unwrap().unwrap();
    let err = arg.pieces().find_map(Result::err).expect("must fail");
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn validate_enforces_the_rules_that_span_the_command_line() {
    let cases: &[(&str, &str)] = &[
        ("p %f %U", "at most one of %f %u %F %U"),
        ("p --files=%U", "%U may only be used as an argument on its own"),
        (r#"p "%U""#, "field codes must not be used inside a quoted argument"),
    ];
    for (exec, why) in cases {
        let src = format!("[Desktop Entry]\nType=Application\nExec={exec}\n");
        let f = parse(src.as_bytes()).unwrap();
        assert_eq!(
            f.exec().unwrap().validate().unwrap_err().kind,
            ErrorKind::Malformed,
            "{why}: {exec}"
        );
    }
}

#[test]
fn an_unterminated_quote_is_rejected() {
    let f = parse(b"[Desktop Entry]\nType=Application\nExec=p \"unclosed\n").unwrap();
    let err = f.exec().unwrap().args().nth(1).unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
}

// ------------------------------------------------------------------ actions

#[test]
fn action_groups_are_found_by_identifier() {
    let bytes = fixture("actions.bin");
    let f = parse(&bytes).unwrap();

    let ids: Vec<String> = f.action_ids().unwrap().map(collect).collect();
    assert_eq!(ids, vec!["new-window", "new-private"]);

    let a = f.action("new-window").expect("[Desktop Action new-window]");
    assert_eq!(collect(a.value("Name").unwrap()), "New Window");
    assert_eq!(a.exec().unwrap().raw(), "/usr/bin/browser --new-window");
    assert!(f.action("does-not-exist").is_none());
}

// -------------------------------------------------------------- malformed

#[test]
fn a_key_before_the_first_group_is_rejected() {
    let bytes = fixture("key-before-group.bin");
    let err = parse(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
    assert_eq!(err.offset, 0, "the orphan is the first line");
}

#[test]
fn an_unterminated_group_header_is_rejected() {
    let bytes = fixture("unterminated-group.bin");
    let err = parse(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
    assert_eq!(err.offset, 0);
}

#[test]
fn an_illegal_key_character_is_rejected() {
    let bytes = fixture("bad-key-char.bin");
    let err = parse(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
    assert_eq!(err.offset, 26, "the offset points at the offending line");
}

#[test]
fn a_group_name_containing_a_bracket_is_rejected() {
    let err = parse(b"[Desktop [Entry]]\nType=Link\n").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn non_utf8_is_reported_with_an_offset() {
    let err = parse(b"[Desktop Entry]\nName=\xff\n").unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidUtf8);
    assert_eq!(err.offset, 21);
}

#[test]
fn a_line_with_no_equals_sign_is_rejected() {
    let err = parse(b"[Desktop Entry]\nType=Link\njunk\n").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
}

// ----------------------------------------------------------------- alloc

#[cfg(feature = "alloc")]
#[test]
fn lossy_unescaping_keeps_an_invalid_sequence_verbatim() {
    let f = parse(b"[Desktop Entry]\nType=Link\nURL=x:y\nComment=a\\qb\\sc\\\n").unwrap();
    let v = f.desktop_entry().unwrap().value("Comment").unwrap();
    assert!(v.to_unescaped().is_err(), "strict decoding still fails");
    assert_eq!(v.to_unescaped_lossy(), "a\\qb c\\", "and the display path keeps going");
}

// ---------------------------------------------------------------- sidecars

/// Read `"expect"` out of a sidecar without a JSON dependency. The sidecars are
/// generated to a fixed shape, and a dev-dependency on `serde_json` to read one
/// field would be the largest dependency in the crate.
fn expect_of(json: &str) -> &str {
    let at = json.find("\"expect\"").expect("sidecar has an expect field");
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
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/rclip-desktop-entry");
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
                let f = parse(&bytes)
                    .unwrap_or_else(|e| panic!("{stem} claims ok but failed: {e}"));
                assert!(
                    f.desktop_entry().is_some(),
                    "{stem}: every well-formed entry has a [Desktop Entry] group"
                );
            }
            "error" => {
                let failed = match stem.as_str() {
                    // parse() is structural; a dangling escape only shows up
                    // when the value is decoded.
                    "unterminated-escape" => parse(&bytes).map_or(true, |f| {
                        f.desktop_entry()
                            .and_then(|g| g.value("Name"))
                            .is_some_and(|v| v.chars().any(|c| c.is_err()))
                    }),
                    _ => parse(&bytes).is_err(),
                };
                assert!(failed, "{stem} claims error but parsed cleanly");
            }
            other => panic!("{stem}: expect must be \"ok\" or \"error\", not {other:?}"),
        }
    }
    assert_eq!(seen, 11, "a new fixture needs a test that says what it means");
}

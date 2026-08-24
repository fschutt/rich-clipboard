//! Integration tests for `text/uri-list` and the Linux cut/copy conventions.
//!
//! RFC 2483 §5 is four sentences long; almost everything worth testing here is
//! about the conventions layered on top of it, none of which is specified. Each
//! assertion names the implementation it was read from.

use rclip_core::ErrorKind;
use rclip_uri_list::{
    convention::{
        parse_copied_files, parse_kde_cut_selection, parse_nautilus_text_clipboard, FileAction,
        MIME_GNOME_COPIED_FILES, MIME_KDE_CUT_SELECTION, MIME_URI_LIST, NAUTILUS_TEXT_MAGIC,
    },
    emit::{self, Payload, RECOMMENDED},
    parse, Entry, ShortcutTarget,
};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-uri-list/"
    );
    std::fs::read(format!("{p}{name}")).expect("fixture")
}

fn uris(bytes: &[u8]) -> Vec<&str> {
    parse(bytes)
        .expect("parses")
        .uris()
        .map(|u| u.as_str())
        .collect()
}

// -------------------------------------------------------------- RFC 2483 §5

#[test]
fn crlf_with_a_trailing_terminator_yields_no_empty_entry() {
    let bytes = fixture("two-files-crlf.bin");
    assert_eq!(
        uris(&bytes),
        vec!["file:///home/me/a.txt", "file:///home/me/b%20c.txt"]
    );
}

#[test]
fn lf_only_is_accepted_because_glib_accepts_it() {
    // g_uri_list_extract_uris: "We also allow LF delimination as well as the
    // specified CRLF."
    let bytes = fixture("two-files-lf.bin");
    assert_eq!(
        uris(&bytes),
        vec!["file:///home/me/a.txt", "file:///home/me/b.txt"]
    );
}

#[test]
fn a_lone_cr_also_separates() {
    // Chromium passes "\r\n" to SplitStringPiece, which treats it as a
    // character set, so a lone CR separates there too.
    assert_eq!(
        uris(b"file:///a\rfile:///b"),
        vec!["file:///a", "file:///b"]
    );
}

#[test]
fn hash_is_only_a_comment_at_the_start_of_a_line() {
    let bytes = fixture("comment-and-uri.bin");
    let entries: Vec<_> = parse(&bytes).unwrap().entries().collect();
    assert_eq!(entries.len(), 2);
    assert!(matches!(entries[0], Entry::Comment { text, .. } if text.contains("rfc:2483")));
    let Entry::Uri(u) = entries[1] else {
        panic!("second entry is a URI")
    };
    assert_eq!(
        u.as_str(),
        "https://example.com/a#frag",
        "a fragment is not a comment - RFC 2483 says so explicitly"
    );
}

#[test]
fn a_trailing_nul_from_qt_3_is_dropped() {
    // qmimedata.cpp: "Qt 3.x will send text/uri-list with a trailing
    // null-terminator ... so chop it off".
    let bytes = fixture("qt-trailing-nul.bin");
    assert_eq!(uris(&bytes), vec!["file:///home/me/a.txt"]);
}

#[test]
fn a_file_uri_splits_into_authority_and_still_encoded_path() {
    let list = parse(b"file:///home/me/a%20b.txt\r\n").unwrap();
    let f = list.first().unwrap().as_file().unwrap();
    assert_eq!(f.host(), "");
    assert!(f.is_local());
    assert_eq!(
        f.path(),
        "/home/me/a%20b.txt",
        "decoding is the caller's decision"
    );
}

#[test]
fn localhost_and_an_authority_less_form_are_both_local() {
    for raw in ["file://localhost/x", "file:/x", "file:///x"] {
        let list = parse(raw.as_bytes()).unwrap();
        let f = list.first().unwrap().as_file().expect("a file URI");
        assert!(f.is_local(), "{raw} names a local file");
        assert_eq!(f.path(), "/x", "{raw}");
    }
    let list = parse(b"file://nas.example/share/x").unwrap();
    let f = list.first().unwrap().as_file().unwrap();
    assert!(!f.is_local(), "a real authority is not this machine");
    assert_eq!(f.host(), "nas.example");
}

#[test]
fn a_non_file_uri_is_not_a_file_uri() {
    let list = parse(b"https://example.com/x").unwrap();
    assert!(list.first().unwrap().as_file().is_none());
    assert_eq!(
        list.first().unwrap().target(),
        ShortcutTarget::Url("https://example.com/x")
    );
}

#[test]
fn percent_decoding_yields_bytes_because_posix_paths_are_bytes() {
    let list = parse(b"file:///tmp/%FF%FE.bin").unwrap();
    let decoded: Vec<u8> = list
        .first()
        .unwrap()
        .percent_decode()
        .map(|b| b.expect("valid escapes"))
        .collect();
    assert_eq!(decoded, b"file:///tmp/\xff\xfe.bin");
}

#[test]
fn a_truncated_percent_escape_is_reported_with_its_offset() {
    let bytes = fixture("bad-percent.bin");
    let list = parse(&bytes).expect("parse is structural and must succeed");
    let err = list.validate_percent_encoding().unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
    assert_eq!(
        err.offset, 17,
        "points at the '%' that has no two hex digits after it"
    );
}

#[test]
fn non_utf8_is_reported_with_an_offset() {
    let bytes = fixture("invalid-utf8.bin");
    let err = parse(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidUtf8);
    assert_eq!(err.offset, 16);
}

// ---------------------------------------------- x-special/gnome-copied-files

#[test]
fn a_gnome_cut_payload_carries_the_verb_and_the_files() {
    let bytes = fixture("gnome-cut.bin");
    let cf = parse_copied_files(&bytes).unwrap();
    assert_eq!(cf.action(), FileAction::Cut);
    assert_eq!(
        cf.uris().map(|u| u.as_str()).collect::<Vec<_>>(),
        vec!["file:///home/me/a.txt", "file:///home/me/b.txt"]
    );
}

#[test]
fn a_gnome_copy_payload_with_one_file_parses() {
    let bytes = fixture("gnome-copy.bin");
    let cf = parse_copied_files(&bytes).unwrap();
    assert_eq!(cf.action(), FileAction::Copy);
    assert_eq!(cf.uris().count(), 1);
}

#[test]
fn a_verb_with_no_files_yields_no_uris() {
    // Nautilus writes exactly "copy" - four bytes, no LF - for an empty
    // selection. One empty URI here would become one bogus paste target.
    let bytes = fixture("gnome-empty.bin");
    let cf = parse_copied_files(&bytes).unwrap();
    assert_eq!(cf.action(), FileAction::Copy);
    assert_eq!(cf.uris().count(), 0);
}

#[test]
fn a_verb_that_does_not_exist_is_bad_magic() {
    // There is no "move" or "link" verb in any emitter. Falling back to copy
    // would leave "move" sitting at the head of the file list.
    let bytes = fixture("gnome-bad-verb.bin");
    let err = parse_copied_files(&bytes).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadMagic);
    assert_eq!(err.offset, 0);
}

#[test]
fn reading_is_lenient_the_way_thunars_reader_is() {
    // thunar-clipboard-manager.c uses g_ascii_strncasecmp on "copy\n"/"cut\n",
    // and passes the rest to g_uri_list_extract_uris, which tolerates CRLF.
    for payload in [
        &b"CUT\nfile:///a"[..],
        &b"cut\r\nfile:///a\r\n"[..],
        &b"cut\nfile:///a\n"[..],
    ] {
        let cf = parse_copied_files(payload).expect("readers are lax");
        assert_eq!(cf.action(), FileAction::Cut, "{payload:?}");
        assert_eq!(cf.uris().count(), 1, "{payload:?}");
    }
}

// ------------------------------------------- legacy x-special/nautilus-clipboard

#[test]
fn the_pre_nautilus_40_text_payload_is_recognized() {
    let bytes = fixture("nautilus-legacy-text.bin");
    assert!(rclip_uri_list::convention::is_nautilus_text_clipboard(
        &bytes
    ));
    let cf = parse_nautilus_text_clipboard(&bytes).unwrap();
    assert_eq!(cf.action(), FileAction::Copy);
    assert_eq!(
        cf.uris().map(|u| u.as_str()).collect::<Vec<_>>(),
        vec!["file:///a", "file:///b"]
    );
}

#[test]
fn the_magic_line_is_not_a_mime_type_and_is_not_offered() {
    assert!(
        !RECOMMENDED.iter().any(|o| o.mime == NAUTILUS_TEXT_MAGIC),
        "Nautilus stopped writing it in version 40 and it was never a real target"
    );
}

#[test]
fn an_ordinary_uri_list_is_not_mistaken_for_the_legacy_payload() {
    let bytes = fixture("two-files-crlf.bin");
    assert!(!rclip_uri_list::convention::is_nautilus_text_clipboard(
        &bytes
    ));
    assert_eq!(
        parse_nautilus_text_clipboard(&bytes).unwrap_err().kind,
        ErrorKind::BadMagic
    );
}

// ------------------------------------- application/x-kde-cutselection

#[test]
fn the_kde_flag_is_read_from_byte_zero_only() {
    // KIO::isClipboardDataCut is `!a.isEmpty() && a.at(0) == '1'`.
    assert_eq!(
        parse_kde_cut_selection(&fixture("kde-cutselection-cut.bin")),
        FileAction::Cut
    );
    assert_eq!(
        parse_kde_cut_selection(&fixture("kde-cutselection-copy.bin")),
        FileAction::Copy
    );
    assert_eq!(
        parse_kde_cut_selection(b"1\n"),
        FileAction::Cut,
        "a stray newline changes nothing"
    );
    assert_eq!(
        parse_kde_cut_selection(b""),
        FileAction::Copy,
        "absent means copy"
    );
    assert_eq!(
        parse_kde_cut_selection(b"true"),
        FileAction::Copy,
        "anything else means copy"
    );
}

#[test]
fn kde_writes_zero_for_copy_so_presence_is_not_cut() {
    assert_eq!(emit::kde_cut_selection(FileAction::Copy), b"0");
    assert_eq!(emit::kde_cut_selection(FileAction::Cut), b"1");
}

// ------------------------------------------------------------------- emit

#[test]
fn the_recommended_offer_set_covers_all_three_desktops() {
    let mimes: Vec<_> = RECOMMENDED.iter().map(|o| o.mime).collect();
    assert!(
        mimes.contains(&MIME_URI_LIST),
        "every reader understands this one"
    );
    assert!(
        mimes.contains(&MIME_GNOME_COPIED_FILES),
        "GNOME, Xfce, Cinnamon, COSMIC"
    );
    assert!(
        mimes.contains(&MIME_KDE_CUT_SELECTION),
        "KDE reads nothing else"
    );
    assert_eq!(
        RECOMMENDED[0].mime, MIME_URI_LIST,
        "advertise the universal one first"
    );
}

#[cfg(feature = "alloc")]
#[test]
fn the_gnome_payload_is_written_without_a_trailing_newline() {
    // Since Nautilus 44 an empty line makes the whole deserialization fail, so
    // a trailing newline is not a cosmetic difference - the paste does nothing.
    let out = emit::write_copied_files(FileAction::Cut, ["file:///a", "file:///b"]);
    assert_eq!(out, b"cut\nfile:///a\nfile:///b");
    assert!(!out.ends_with(b"\n"));
    assert!(
        !out.windows(2).any(|w| w == b"\r\n"),
        "CRLF fails g_uri_is_valid"
    );
}

#[cfg(feature = "alloc")]
#[test]
fn the_uri_list_payload_is_written_with_crlf_after_every_uri() {
    // GTK's file_uri_serializer and Qt's retrieveTypedData both append after
    // the last URI too.
    let out = emit::write_uri_list(["file:///a", "file:///b"]);
    assert_eq!(out, b"file:///a\r\nfile:///b\r\n");
}

#[cfg(feature = "alloc")]
#[test]
fn what_is_written_is_what_is_read_back() {
    for action in [FileAction::Copy, FileAction::Cut] {
        let want = ["file:///home/me/a%20b.txt", "file:///home/me/c.txt"];
        let bytes = emit::write_copied_files(action, want);
        let cf = parse_copied_files(&bytes).unwrap();
        assert_eq!(cf.action(), action);
        assert_eq!(
            cf.uris().map(|u| u.as_str()).collect::<Vec<_>>(),
            want.to_vec()
        );

        let bytes = emit::write_uri_list(want);
        assert_eq!(uris(&bytes), want.to_vec());
    }
}

#[cfg(feature = "alloc")]
#[test]
fn write_dispatches_on_the_payload_kind() {
    let files = ["file:///a"];
    for offer in RECOMMENDED {
        let out = emit::write(offer, FileAction::Cut, &files);
        match offer.payload {
            Payload::UriList => assert_eq!(out, b"file:///a\r\n"),
            Payload::CopiedFiles => assert_eq!(out, b"cut\nfile:///a"),
            Payload::KdeCutSelection => assert_eq!(out, b"1"),
            _ => panic!("unhandled payload kind"),
        }
    }
}

#[cfg(feature = "alloc")]
#[test]
fn decoding_to_a_string_fails_loudly_on_a_non_utf8_path() {
    let list = parse(b"file:///tmp/%FF.bin").unwrap();
    let u = list.first().unwrap();
    assert_eq!(u.to_decoded_bytes().unwrap(), b"file:///tmp/\xff.bin");
    assert_eq!(
        u.to_decoded_string().unwrap_err().kind,
        ErrorKind::InvalidUtf8
    );
}

// -------------------------------------------------------- percent-encoding

#[test]
fn the_path_encode_set_is_rfc_3986_pchar_plus_slash() {
    use rclip_uri_list::EncodeSet;

    // `pchar = unreserved / pct-encoded / sub-delims / ":" / "@"`, plus the
    // separator. This is also, byte for byte, GLib's
    // G_URI_RESERVED_CHARS_ALLOWED_IN_PATH, which is what g_filename_to_uri
    // escapes with — matching it is what lets the receiving side compare URIs.
    for b in b"-._~!$&'()*+,;=:@/" {
        assert!(
            EncodeSet::Path.allows(*b),
            "{:?} is legal in a path and escaping it is over-encoding",
            *b as char
        );
    }
    for b in b" \"#%<>?[]\\^`{|}" {
        assert!(
            !EncodeSet::Path.allows(*b),
            "{:?} changes what the URI means and must be escaped",
            *b as char
        );
    }
    // Control bytes and everything non-ASCII.
    assert!(!EncodeSet::Path.allows(0x00));
    assert!(!EncodeSet::Path.allows(0x1F));
    assert!(!EncodeSet::Path.allows(0x7F));
    assert!(!EncodeSet::Path.allows(0xC3));

    // A segment is a pchar run with no separator in it.
    assert!(!EncodeSet::Segment.allows(b'/'));
    assert!(EncodeSet::Segment.allows(b'&'));

    // Unreserved keeps only the four punctuation marks §2.3 names.
    for b in b"-._~" {
        assert!(EncodeSet::Unreserved.allows(*b));
    }
    for b in b"!$&'()*+,;=:@/" {
        assert!(!EncodeSet::Unreserved.allows(*b));
    }
}

#[test]
fn a_literal_percent_is_never_left_alone() {
    use rclip_uri_list::EncodeSet;

    // The one byte where getting it wrong silently corrupts a filename: leave
    // it and the next reader takes the two bytes after it for an escape.
    for set in [EncodeSet::Path, EncodeSet::Segment, EncodeSet::Unreserved] {
        assert!(!set.allows(b'%'), "{set:?} must escape a literal %");
    }
}

#[test]
fn encoding_allocates_nothing_and_yields_ascii() {
    use core::fmt::Write as _;
    use rclip_uri_list::{percent_encode, EncodeSet};

    let enc = percent_encode(b"/tmp/a b\xffc", EncodeSet::Path);
    // `PercentEncode` is `Copy`, so each of these consumes its own copy.
    assert!(
        { enc }.all(|b| b.is_ascii()),
        "the output has to be embeddable in a &str"
    );
    // `encoded_len` has to agree with what the iterator actually produces.
    assert_eq!(
        rclip_uri_list::uri::encoded_len(b"/tmp/a b\xffc", EncodeSet::Path),
        { enc }.count()
    );

    // Display straight into a fmt::Write, no allocator involved.
    let mut sink = String::new();
    write!(sink, "{enc}").unwrap();
    assert_eq!(sink, "/tmp/a%20b%FFc");
}

#[test]
fn hex_digits_are_uppercase() {
    use rclip_uri_list::{percent_encode, EncodeSet};

    // RFC 3986 §2.1 says "should use uppercase", and GLib, Qt and Chromium all
    // do — a lowercase escape is a textual mismatch for a reader that compares
    // URIs without normalising them.
    let s = percent_encode(b"\xab\xcd \x0a", EncodeSet::Path).to_string();
    assert_eq!(s, "%AB%CD%20%0A");
}

#[cfg(feature = "alloc")]
#[test]
fn a_path_becomes_the_uri_glib_would_have_produced() {
    use rclip_uri_list::emit::file_uri;

    assert_eq!(file_uri("/home/me/notes.txt"), "file:///home/me/notes.txt");
    assert_eq!(
        file_uri("/home/me/a file.txt"),
        "file:///home/me/a%20file.txt"
    );
    // The under-encoding trap: an unescaped `#` makes the rest a fragment, so
    // the file would arrive as `/tmp/notes`.
    assert_eq!(file_uri("/tmp/notes#2.txt"), "file:///tmp/notes%232.txt");
    assert_eq!(file_uri("/tmp/q?.txt"), "file:///tmp/q%3F.txt");
    // A literal percent must double.
    assert_eq!(file_uri("/tmp/100%.txt"), "file:///tmp/100%25.txt");
    // The over-encoding trap: sub-delims are legal and must stay literal.
    assert_eq!(
        file_uri("/tmp/it's (1)&2,3;4=5+6!7$8*9@a:b.txt"),
        "file:///tmp/it's%20(1)&2,3;4=5+6!7$8*9@a:b.txt"
    );
    // Non-UTF-8 bytes survive, because a POSIX path is a byte string.
    assert_eq!(file_uri(&b"/tmp/\xff.bin"[..]), "file:///tmp/%FF.bin");
}

#[cfg(feature = "alloc")]
#[test]
fn a_path_without_a_leading_slash_does_not_become_a_hostname() {
    use rclip_uri_list::emit::file_uri;

    // `file://home/me` parses `home` as an authority, which is the one
    // malformation that changes what the URI *means* rather than how it looks.
    let uri = file_uri("home/me/x.txt");
    assert_eq!(uri, "file:///home/me/x.txt");

    let list = parse(uri.as_bytes()).unwrap();
    let f = list.first().unwrap().as_file().unwrap();
    assert!(f.host().is_empty(), "the authority must stay empty");
    assert_eq!(f.path(), "/home/me/x.txt");
}

#[cfg(feature = "alloc")]
#[test]
fn every_encoded_path_round_trips_through_the_decoder() {
    use rclip_uri_list::emit::file_uri;

    let paths: &[&[u8]] = &[
        b"/home/me/plain.txt",
        b"/home/me/a file with spaces.txt",
        b"/tmp/hash#and?query&amp.txt",
        b"/tmp/100% sure.txt",
        b"/tmp/\xff\xfe not utf-8",
        b"/tmp/[brackets]{braces}.txt",
        b"/tmp/back\\slash.txt",
        b"/tmp/\x01control.txt",
        b"/tmp/caf\xc3\xa9.txt",
    ];
    for path in paths {
        let uri = file_uri(path);
        let list = parse(uri.as_bytes()).expect("an encoded URI must be UTF-8");
        list.validate_percent_encoding()
            .unwrap_or_else(|e| panic!("{uri} does not validate: {e}"));
        let u = list.first().unwrap();
        assert_eq!(
            u.to_decoded_bytes().unwrap(),
            [b"file://".as_slice(), path].concat(),
            "{uri} did not decode back to the path it came from"
        );
        // And through the part a caller actually uses.
        let f = u.as_file().unwrap();
        assert!(f.is_local());
        let decoded: Vec<u8> = parse(f.path().as_bytes())
            .unwrap()
            .first()
            .unwrap()
            .to_decoded_bytes()
            .unwrap();
        assert_eq!(decoded, path.to_vec());
    }
}

#[cfg(feature = "alloc")]
#[test]
fn an_encoded_path_survives_a_whole_uri_list_and_a_gnome_payload() {
    use rclip_uri_list::emit::{self as e, file_uri};

    let paths = ["/tmp/a b.txt", "/tmp/c#d.txt"];
    let encoded: Vec<String> = paths.iter().map(file_uri).collect();
    let refs: Vec<&str> = encoded.iter().map(String::as_str).collect();

    assert_eq!(uris(&e::write_uri_list(refs.iter().copied())), refs);

    let bytes = e::write_copied_files(FileAction::Cut, refs.iter().copied());
    let cf = parse_copied_files(&bytes).unwrap();
    assert_eq!(cf.action(), FileAction::Cut);
    assert_eq!(cf.uris().map(|u| u.as_str()).collect::<Vec<_>>(), refs);
    assert!(
        !bytes.windows(1).any(|w| w == b" "),
        "an unencoded space is what makes g_uri_is_valid reject the payload"
    );
}

#[cfg(feature = "alloc")]
#[test]
fn encoding_a_uri_that_is_already_one_would_double_its_escapes() {
    use rclip_uri_list::{percent_encode_to_string, EncodeSet};

    // Why `write_uri_list` passes URIs through verbatim rather than escaping
    // them: this is what the alternative does.
    assert_eq!(
        percent_encode_to_string(b"file:///tmp/a%20b.txt", EncodeSet::Path),
        "file:///tmp/a%2520b.txt"
    );
}

#[cfg(feature = "alloc")]
#[test]
fn the_reserved_set_fixture_decodes_back_to_the_paths_it_names() {
    use rclip_uri_list::emit::file_uri;

    // Hand-written to the RFC rather than generated here, so this is a real
    // check on the encoder and not a restatement of it.
    let bytes = fixture("encoded-reserved-chars.bin");
    let list = parse(&bytes).unwrap();
    list.validate_percent_encoding().unwrap();

    let want: &[&[u8]] = &[
        b"/tmp/a file.txt",
        b"/tmp/notes#2.txt",
        b"/tmp/100%.txt",
        b"/tmp/it's (1)&2,3;4=5+6!7$8*9@a:b.txt",
        b"/tmp/\xff.bin",
        b"/tmp/caf\xc3\xa9.txt",
    ];
    let got: Vec<Vec<u8>> = list
        .uris()
        .map(|u| {
            let path = u.as_file().unwrap().path();
            parse(path.as_bytes())
                .unwrap()
                .first()
                .unwrap()
                .to_decoded_bytes()
                .unwrap()
        })
        .collect();
    assert_eq!(got, want.to_vec());

    // And the encoder reproduces the fixture byte for byte.
    let rebuilt: Vec<String> = want.iter().map(file_uri).collect();
    assert_eq!(
        rclip_uri_list::emit::write_uri_list(rebuilt.iter().map(String::as_str)),
        bytes,
        "this crate must produce exactly the bytes the RFC-derived fixture holds"
    );
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
        "/../../corpus/synthetic/rclip-uri-list"
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
                // The convention payloads are not uri-lists on their own, so the
                // check is per family.
                if stem.starts_with("gnome-") {
                    parse_copied_files(&bytes)
                        .unwrap_or_else(|e| panic!("{stem} claims ok but failed: {e}"));
                } else if stem.starts_with("nautilus-") {
                    parse_nautilus_text_clipboard(&bytes)
                        .unwrap_or_else(|e| panic!("{stem} claims ok but failed: {e}"));
                } else if stem.starts_with("kde-") {
                    // Infallible by construction; assert it agrees with the name.
                    let want = if stem.ends_with("cut") {
                        FileAction::Cut
                    } else {
                        FileAction::Copy
                    };
                    assert_eq!(parse_kde_cut_selection(&bytes), want, "{stem}");
                } else {
                    let list = parse(&bytes)
                        .unwrap_or_else(|e| panic!("{stem} claims ok but failed: {e}"));
                    list.validate_percent_encoding()
                        .unwrap_or_else(|e| panic!("{stem} has bad percent-encoding: {e}"));
                }
            }
            "error" => {
                let failed = match stem.as_str() {
                    // parse() is structural; a truncated escape is only visible
                    // once a URI is looked at.
                    "bad-percent" => {
                        parse(&bytes).map_or(true, |l| l.validate_percent_encoding().is_err())
                    }
                    s if s.starts_with("gnome-") => parse_copied_files(&bytes).is_err(),
                    _ => parse(&bytes).is_err(),
                };
                assert!(failed, "{stem} claims error but parsed cleanly");
            }
            other => panic!("{stem}: expect must be \"ok\" or \"error\", not {other:?}"),
        }
    }
    assert_eq!(
        seen, 14,
        "a new fixture needs a test that says what it means"
    );
}

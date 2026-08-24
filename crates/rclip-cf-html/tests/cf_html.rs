//! Integration tests for `rclip-cf-html`, driven by `corpus/synthetic/rclip-cf-html`.
//!
//! The tests that matter most here are the ones about *disagreement*: between
//! the marker comments and the byte offsets, between the spec and its own
//! examples, and between what a producer claims and what is actually in the
//! buffer.

use rclip_cf_html::{
    parse, parse_detailed, CfHtmlBuilder, ErrorKind, FragmentSource, Offset, Version,
};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/");
    std::fs::read(format!("{p}rclip-cf-html/{name}")).expect("fixture")
}

fn fixture_names() -> Vec<String> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-cf-html"
    );
    let mut out: Vec<String> = std::fs::read_dir(p)
        .expect("corpus dir")
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            name.ends_with(".bin").then_some(name)
        })
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------- fixtures

#[test]
fn mshtml_example_trusts_the_comments_over_microsofts_own_numbers() {
    let blob = fixture("mshtml-scenario1.bin");
    let p = parse_detailed(&blob).expect("the spec's own example must parse");

    assert_eq!(p.content.version, Version::V1_0);
    assert_eq!(
        p.content.fragment,
        "<body>This is normal. <b>This is bold.</b> \
         <i><b>This is bold italic.</b> This is italic.</i></body>",
        "the fragment is what the marker comments bracket, at 147..247"
    );

    // The numbers Microsoft published. Kept verbatim so this test fails loudly
    // if anyone "fixes" the fixture.
    assert_eq!(p.header.start_fragment, Offset::At(6));
    assert_eq!(p.header.end_fragment, Offset::At(106));
    assert_eq!(
        p.fragment_source,
        FragmentSource::CommentsOverrodeOffsets,
        "the disagreement must be reported, not silently swallowed"
    );

    // StartHTML/EndHTML in the same example *are* right.
    assert_eq!(p.header.start_html, Offset::At(121));
    assert_eq!(p.header.end_html, Offset::At(272));
    assert_eq!(
        p.header.header_len, 121,
        "the parser must agree with StartHTML here"
    );
    assert!(p
        .content
        .context
        .unwrap()
        .starts_with("<html><!--StartFragment-->"));
    assert!(p.content.context.unwrap().ends_with("</html>"));

    // So is the selection, which lands mid-tag on purpose: the spec says a
    // selection is a raw text range and need not be well-formed HTML.
    assert_eq!(
        p.content.selection,
        Some("bold.</b> <i><b>This is bold italic.</b> This"),
        "the selection is the exact highlighted run, unbalanced tags and all"
    );
    assert_eq!(p.selection_in_fragment, Some((33, 78)));
    assert_eq!(p.content.source_url, None);
}

#[test]
fn zero_padded_offsets_parse_and_agree() {
    let blob = fixture("zero-padded-offsets.bin");
    let p = parse_detailed(&blob).unwrap();
    assert_eq!(p.content.fragment, "<p>Hello, <b>wörld</b> — café</p>");
    assert_eq!(
        p.content.source_url,
        Some("https://example.org/article?q=1#frag")
    );
    assert_eq!(p.content.version, Version::V0_9);
    assert_eq!(
        p.fragment_source,
        FragmentSource::Agreed,
        "a well-formed producer must not be reported as disagreeing"
    );
    assert_eq!(p.header.start_html, Offset::At(p.header.header_len));
    assert_eq!(p.header.end_html, Offset::At(blob.len()));
}

#[test]
fn minus_one_means_fragment_only() {
    let blob = fixture("negative-one-no-context.bin");
    let p = parse_detailed(&blob).unwrap();
    assert_eq!(p.header.start_html, Offset::Negative);
    assert_eq!(p.header.end_html, Offset::Negative);
    assert_eq!(
        p.content.context, None,
        "-1 means there is no context to return"
    );
    assert_eq!(
        p.content.fragment,
        r#"<span style="color:#c00">fragment only</span>"#
    );
    assert_eq!(p.fragment_source, FragmentSource::Agreed);
}

#[test]
fn an_offset_past_the_end_is_an_error_not_a_panic() {
    let blob = fixture("offsets-past-end.bin");
    let err = parse(&blob).expect_err("EndHTML names bytes that do not exist");
    assert_eq!(err.kind, ErrorKind::BadOffset);
    assert_eq!(
        err.offset, 9_999_999_999,
        "the error must carry the offending offset"
    );
}

#[test]
fn lone_carriage_returns_terminate_header_lines() {
    let blob = fixture("lone-cr-line-ends.bin");
    assert!(
        !blob[..100].contains(&b'\n'),
        "fixture must really use bare CRs"
    );
    let p = parse_detailed(&blob).unwrap();
    assert_eq!(p.content.fragment, "<em>archaic</em>");
    assert_eq!(p.fragment_source, FragmentSource::Agreed);
}

#[test]
fn unknown_keys_and_unknown_versions_are_accepted() {
    let blob = fixture("unknown-keys-lf.bin");
    let p = parse_detailed(&blob).unwrap();
    assert_eq!(
        p.content.version,
        Version::Other("1.1"),
        "an unrecognised version must be carried through, not rejected"
    );
    assert_eq!(
        p.content.source_url,
        Some("about:blank"),
        "the value keeps its own colons; only the first one is the separator"
    );
    assert_eq!(p.content.fragment, "<h1>Title</h1>");
    assert_eq!(
        p.header.header_len, 167,
        "the unknown X-Future-Key line is part of the header"
    );
}

#[test]
fn half_a_selection_pair_is_rejected() {
    let blob = fixture("selection-half-present.bin");
    let err = parse(&blob).expect_err("StartSelection without EndSelection");
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn offsets_alone_are_used_when_there_are_no_marker_comments() {
    let blob = fixture("no-fragment-markers.bin");
    let p = parse_detailed(&blob).unwrap();
    assert_eq!(p.content.fragment, "<p>trust the numbers</p>");
    assert_eq!(
        p.fragment_source,
        FragmentSource::OffsetsOnly,
        "a caller must be able to tell an uncorroborated fragment from a checked one"
    );
}

#[test]
fn comments_alone_are_used_when_the_offsets_are_missing() {
    // A producer that emitted the marker comments but no StartFragment/
    // EndFragment. There is nothing to disagree with, and that is worth
    // distinguishing from a payload where the two sources corroborated.
    let blob = b"Version:0.9\r\nStartHTML:-1\r\nEndHTML:-1\r\n\
<!--StartFragment--><p>uncontested</p><!--EndFragment-->";
    let p = parse_detailed(blob).unwrap();
    assert_eq!(p.content.fragment, "<p>uncontested</p>");
    assert_eq!(p.fragment_source, FragmentSource::CommentsOnly);
    assert_eq!(p.header.start_fragment, Offset::Absent);
}

#[test]
fn bare_html_is_not_cf_html() {
    let blob = fixture("bare-html-no-header.bin");
    let err = parse(&blob).expect_err("no Version: line");
    assert_eq!(err.kind, ErrorKind::BadMagic);
    assert_eq!(err.offset, 0);
}

#[test]
fn every_fixture_matches_its_sidecar() {
    for name in fixture_names() {
        let blob = fixture(&name);
        let sidecar_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../corpus/synthetic/rclip-cf-html/"
        );
        let sidecar = std::fs::read_to_string(format!(
            "{sidecar_path}{}.json",
            name.strip_suffix(".bin").unwrap()
        ))
        .unwrap_or_else(|_| panic!("{name} has no .json sidecar"));

        let expects_ok = sidecar.contains(r#""expect": "ok""#);
        let expects_err = sidecar.contains(r#""expect": "error""#);
        assert!(
            expects_ok ^ expects_err,
            "{name}: sidecar must declare exactly one expectation"
        );

        match parse(&blob) {
            Ok(_) => assert!(
                expects_ok,
                "{name}: parsed, but the sidecar says it should not"
            ),
            Err(e) => {
                assert!(
                    expects_err,
                    "{name}: failed with {e}, but the sidecar says ok"
                );
                // When the sidecar names a kind, it has to be the right one.
                let kind = format!(r#""expect_error_kind": "{:?}""#, e.kind);
                assert!(
                    !sidecar.contains(r#""expect_error_kind""#) || sidecar.contains(&kind),
                    "{name}: got {:?}, which the sidecar does not name",
                    e.kind
                );
            }
        }
    }
}

#[test]
fn no_truncation_of_any_fixture_panics() {
    // Clipboard payloads arrive over pipes and shared memory; a short read is
    // the single most likely corruption. Every prefix of every fixture must
    // come back as Ok or Err, never as a panic.
    for name in fixture_names() {
        let blob = fixture(&name);
        for cut in 0..=blob.len() {
            let _ = parse_detailed(&blob[..cut]);
        }
    }
}

#[test]
fn no_single_byte_corruption_of_any_fixture_panics() {
    for name in fixture_names() {
        let mut blob = fixture(&name);
        for i in 0..blob.len() {
            let original = blob[i];
            for replacement in [0x00, 0x2d, 0x39, 0x3a, 0x0d, 0x0a, 0x80, 0xff] {
                blob[i] = replacement;
                let _ = parse_detailed(&blob);
            }
            blob[i] = original;
        }
    }
}

// ------------------------------------------------------------- serializing

const FRAGMENT: &str = "<p>Round <b>trip</b> — with a ✓ in it</p>";

#[test]
fn round_trip_recovers_every_field() {
    let selection = 3..21;
    let build = || {
        CfHtmlBuilder::new(FRAGMENT)
            .version(Version::V1_0)
            .context(
                "<html><head><base href=\"https://example.com/\"></head><body>",
                "</body></html>",
            )
            .source_url("https://example.com/page?x=1")
            .selection(selection.clone())
            .build()
            .unwrap()
    };

    let bytes = build();
    let x = parse(&bytes).expect("what we wrote must parse");

    assert_eq!(x.version, Version::V1_0);
    assert_eq!(x.fragment, FRAGMENT);
    assert_eq!(x.source_url, Some("https://example.com/page?x=1"));
    assert_eq!(x.selection, Some(&FRAGMENT[selection.clone()]));
    let context = x.context.expect("a context was requested");
    assert!(context.starts_with("<html><head><base"));
    assert!(context.ends_with("</body></html>"));
    assert!(context.contains(FRAGMENT));

    // parse(serialize(x)) == x, on the nose.
    assert_eq!(build(), bytes, "serialization must be deterministic");
    assert_eq!(parse(&build()).unwrap(), x);
}

#[test]
fn what_we_write_needs_no_correction_to_read_back() {
    let bytes = CfHtmlBuilder::new(FRAGMENT).build().unwrap();
    let p = parse_detailed(&bytes).unwrap();
    assert_eq!(
        p.fragment_source,
        FragmentSource::Agreed,
        "our own offsets must match our own comments, or the back-patch is wrong"
    );
    assert_eq!(p.header.start_html, Offset::At(p.header.header_len));
}

#[test]
fn the_header_is_the_same_size_whatever_the_payload() {
    // This is the whole point of the fixed-width placeholder: the offsets are
    // self-referential, and a header whose length depends on the numbers in it
    // is the thing that forces an implementation to iterate.
    let tiny = CfHtmlBuilder::new("x").build().unwrap();
    let huge_fragment = "y".repeat(200_000);
    let huge = CfHtmlBuilder::new(&huge_fragment).build().unwrap();

    let tiny_header = parse_detailed(&tiny).unwrap().header.header_len;
    let huge_header = parse_detailed(&huge).unwrap().header.header_len;
    assert_eq!(
        tiny_header, huge_header,
        "a 1-byte and a 200 KB fragment must produce headers of identical length"
    );

    // And the big one has to still be right, with six-digit offsets.
    let p = parse_detailed(&huge).unwrap();
    assert_eq!(p.content.fragment.len(), 200_000);
    assert_eq!(p.fragment_source, FragmentSource::Agreed);
    assert_eq!(p.header.end_html, Offset::At(huge.len()));
}

#[test]
fn every_offset_field_is_exactly_ten_digits() {
    let bytes = CfHtmlBuilder::new("x")
        .selection(0..1)
        .source_url("u")
        .build()
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    for key in [
        "StartHTML",
        "EndHTML",
        "StartFragment",
        "EndFragment",
        "StartSelection",
        "EndSelection",
    ] {
        let at = text
            .find(&format!("{key}:"))
            .unwrap_or_else(|| panic!("{key} missing"));
        let value = &text[at + key.len() + 1..][..10];
        assert!(
            value.bytes().all(|b| b.is_ascii_digit()),
            "{key} must be ten zero-padded digits, got {value:?}"
        );
        assert_eq!(
            &text[at + key.len() + 11..at + key.len() + 13],
            "\r\n",
            "{key} field is 10 wide"
        );
    }
}

#[test]
fn no_context_writes_minus_one_and_reads_back_as_none() {
    let bytes = CfHtmlBuilder::new("<i>alone</i>")
        .no_context()
        .build()
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("StartHTML:-1\r\n"),
        "-1 is written literally, not zero-padded"
    );
    assert!(text.contains("EndHTML:-1\r\n"));

    let p = parse_detailed(&bytes).unwrap();
    assert_eq!(p.content.context, None);
    assert_eq!(p.content.fragment, "<i>alone</i>");
    assert_eq!(p.fragment_source, FragmentSource::Agreed);
}

#[test]
fn the_default_version_is_the_one_everything_accepts() {
    let bytes = CfHtmlBuilder::new("x").build().unwrap();
    assert!(bytes.starts_with(b"Version:0.9\r\n"));
}

#[test]
fn a_fragment_carrying_a_marker_comment_is_refused() {
    // Otherwise the injected comment, not ours, becomes the boundary every
    // parser downstream believes.
    for evil in [
        "<p>a</p><!--EndFragment--><p>b</p>",
        "<!--StartFragment--><p>a</p>",
        "<p>a</p><!-- EndFragment -->",
    ] {
        let err = CfHtmlBuilder::new(evil)
            .build()
            .expect_err("marker smuggled into the fragment");
        assert_eq!(err.kind, ErrorKind::Malformed, "for {evil:?}");
    }
    let err = CfHtmlBuilder::new("<p>ok</p>")
        .context("<html><!--StartFragment-->", "</html>")
        .build()
        .expect_err("marker smuggled into the context");
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn a_newline_in_a_header_value_is_refused() {
    // A `\n` in the SourceURL would open a header line of the attacker's
    // choosing, which is how you forge a StartFragment.
    let err = CfHtmlBuilder::new("x")
        .source_url("https://e.example/\r\nStartFragment:0000000000")
        .build()
        .expect_err("header injection");
    assert_eq!(err.kind, ErrorKind::Malformed);

    let err = CfHtmlBuilder::new("x")
        .version(Version::Other("1.0\r\nEndHTML:0"))
        .build()
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Malformed);
}

#[test]
fn a_selection_off_a_character_boundary_is_refused() {
    // "wörld" — byte 2 is inside the two-byte o-umlaut.
    let err = CfHtmlBuilder::new("wörld")
        .selection(0..2)
        .build()
        .expect_err("mid-codepoint");
    assert_eq!(err.kind, ErrorKind::BadOffset);

    let backwards = std::ops::Range { start: 2, end: 1 };
    let err = CfHtmlBuilder::new("abc")
        .selection(backwards)
        .build()
        .expect_err("backwards");
    assert_eq!(err.kind, ErrorKind::BadOffset);

    let err = CfHtmlBuilder::new("abc")
        .selection(0..9)
        .build()
        .expect_err("past the end");
    assert_eq!(err.kind, ErrorKind::BadOffset);
}

#[test]
fn rewriting_the_broken_mshtml_example_produces_a_consistent_blob() {
    // The end-to-end job this crate exists for: read a payload whose numbers
    // are wrong, hand the caller the right text, and write it back out so the
    // next consumer does not have to know.
    let blob = fixture("mshtml-scenario1.bin");
    let p = parse_detailed(&blob).unwrap();
    assert_eq!(p.fragment_source, FragmentSource::CommentsOverrodeOffsets);

    let (sel_start, sel_end) = p.selection_in_fragment.unwrap();
    let fixed = CfHtmlBuilder::new(p.content.fragment)
        .version(p.content.version)
        .selection(sel_start..sel_end)
        .build()
        .unwrap();

    let q = parse_detailed(&fixed).unwrap();
    assert_eq!(
        q.fragment_source,
        FragmentSource::Agreed,
        "the rewrite must be self-consistent"
    );
    assert_eq!(q.content.fragment, p.content.fragment);
    assert_eq!(q.content.selection, p.content.selection);
    assert_eq!(q.content.version, Version::V1_0);
}

// ------------------------------------------------------------ hostile input

#[test]
fn empty_input_is_rejected_without_reading_anything() {
    let err = parse(b"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadMagic);
}

#[test]
fn a_header_with_no_way_to_find_the_fragment_is_malformed() {
    let blob = b"Version:0.9\r\nStartHTML:-1\r\nEndHTML:-1\r\n<p>where does this start?</p>";
    let err = parse(blob).unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::Malformed,
        "no comments and no offsets means the fragment is unlocatable"
    );
}

#[test]
fn a_body_that_is_not_utf8_is_rejected_at_the_offending_byte() {
    let mut blob = fixture("zero-padded-offsets.bin");
    let at = blob.len() - 20;
    blob[at] = 0xff;
    let err = parse(&blob).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidUtf8);
    assert!(
        err.offset <= at,
        "the error should point at or before the bad byte"
    );
}

#[test]
fn a_body_line_shaped_like_a_header_is_not_eaten() {
    // Without the "stop at StartHTML" rule, `Note:` would be swallowed as an
    // unknown header key and the fragment would start one line late.
    let body = "<!--StartFragment-->Note: read this<!--EndFragment-->";
    let header = String::from("Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\n");
    let header_len = header.len();
    let header = header
        .replace(
            "StartHTML:0000000000",
            &format!("StartHTML:{header_len:010}"),
        )
        .replace(
            "EndHTML:0000000000",
            &format!("EndHTML:{:010}", header_len + body.len()),
        );
    let blob = format!("{header}{body}");
    let p = parse_detailed(blob.as_bytes()).unwrap();
    assert_eq!(p.content.fragment, "Note: read this");
    assert_eq!(p.header.header_len, header_len);
}

#[test]
fn an_offset_that_would_reach_back_into_the_header_is_clamped() {
    // A producer that under-reports StartHTML must not cause header bytes to
    // be pasted into a document as if they were markup.
    let blob = b"Version:0.9\r\nStartHTML:0000000002\r\nEndHTML:0000000075\r\n\
<html><body><!--StartFragment-->hi<!--EndFragment--></body></html>";
    let p = parse_detailed(blob).unwrap();
    assert_eq!(p.content.fragment, "hi");
    let context = p.content.context.unwrap();
    assert!(
        !context.contains("Version:"),
        "the context must not include header bytes, got {context:?}"
    );
    assert_eq!(
        p.header.start_html,
        Offset::At(2),
        "the raw claim is still reported so a caller can see it was wrong"
    );
}

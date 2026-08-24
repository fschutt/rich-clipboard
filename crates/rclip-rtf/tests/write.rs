//! Integration tests for the RTF writer.
//!
//! The primary property is that this crate's own parser reads back what this
//! crate's writer produced. Everything else here is one of the four ways an RTF
//! writer silently corrupts text rather than failing: a raw high byte, a
//! fallback character that does not match the declared `\ucN`, `\fsN` in points
//! instead of half-points, and a `\colortbl` whose first entry is not the empty
//! "auto" one.

use std::fs;

use rclip_rtf::{
    half_points, write, CharProps, Color, Document, FontFamily, Run, WriteProps, Writer,
};

const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/synthetic/rclip-rtf/"
);

fn fixture(name: &str) -> Vec<u8> {
    fs::read(format!("{CORPUS}{name}")).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

/// What a run looks like once its indices have been resolved through the
/// tables, which is the only form two documents can be compared in.
#[derive(Debug, PartialEq)]
struct Resolved {
    text: String,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    size_half_points: u16,
    font: String,
    fg: Option<Color>,
    bg: Option<Color>,
}

fn resolve(doc: &Document) -> Vec<Resolved> {
    doc.runs
        .iter()
        .map(|r| Resolved {
            text: doc.run_text(r).to_owned(),
            bold: r.props.bold,
            italic: r.props.italic,
            underline: r.props.underline,
            strike: r.props.strike,
            size_half_points: r.props.size_half_points,
            font: r
                .props
                .font
                .and_then(|id| doc.font(id))
                .map(|f| f.name.clone())
                .unwrap_or_default(),
            fg: r.props.foreground.and_then(|i| doc.color(i)),
            bg: r.props.background.and_then(|i| doc.color(i)),
        })
        .collect()
}

fn styled() -> Vec<(&'static str, WriteProps<'static>)> {
    alloc_runs()
}

fn alloc_runs() -> Vec<(&'static str, WriteProps<'static>)> {
    vec![
        ("plain ", WriteProps::default()),
        (
            "bold red ",
            WriteProps {
                bold: true,
                foreground: Some(Color::new(255, 0, 0)),
                ..WriteProps::default()
            },
        ),
        (
            "italic underline strike ",
            WriteProps {
                italic: true,
                underline: true,
                strike: true,
                ..WriteProps::default()
            },
        ),
        (
            "Courier 18pt on yellow",
            WriteProps {
                font: Some("Courier New"),
                size_half_points: Some(half_points(18.0)),
                background: Some(Color::new(255, 255, 0)),
                ..WriteProps::default()
            },
        ),
    ]
}

// ------------------------------------------------------------- round trip

#[test]
fn the_parser_reads_back_what_the_writer_wrote() {
    let bytes = write(styled());
    let doc = Document::parse(&bytes).expect("our own output must parse");
    let got = resolve(&doc);

    assert_eq!(
        doc.text,
        "plain bold red italic underline strike Courier 18pt on yellow"
    );
    assert_eq!(got.len(), 4, "one run per distinct formatting: {got:#?}");

    assert!(got[1].bold);
    assert_eq!(got[1].fg, Some(Color::new(255, 0, 0)));
    assert_eq!(got[1].bg, None);
    assert!(got[2].italic && got[2].underline && got[2].strike);
    assert_eq!(got[3].font, "Courier New");
    assert_eq!(got[3].size_half_points, 36, "18pt is 36 half-points");
    assert_eq!(got[3].bg, Some(Color::new(255, 255, 0)));

    // And it is stable: writing what came back gives the same bytes.
    let again = write(styled());
    assert_eq!(bytes, again);
}

#[test]
fn a_parsed_document_round_trips_through_to_rtf_exactly() {
    // Not "equivalently" — equally. `to_rtf` writes the tables back verbatim
    // so `\f3` still means `\f3` and colour 2 is still colour 2; a writer that
    // renumbered them would produce a document that renders the same and
    // compares different, and the difference is impossible to review.
    for name in [
        "minimal.bin",
        "font-color-table.bin",
        "unicode-uc2.bin",
        "surrogate-pair.bin",
        "nested-unknown-dest.bin",
        "hex-escapes-cp1252.bin",
    ] {
        let doc = Document::parse(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let rewritten = doc.to_rtf();
        let again = Document::parse(&rewritten).unwrap_or_else(|e| {
            panic!(
                "{name} rewritten: {e}\n{}",
                String::from_utf8_lossy(&rewritten)
            )
        });
        assert_eq!(again.text, doc.text, "{name}: text");
        assert_eq!(again.runs, doc.runs, "{name}: runs");
        assert_eq!(again.fonts, doc.fonts, "{name}: fonts");
        assert_eq!(again.colors, doc.colors, "{name}: colors");
        assert_eq!(again.default_font, doc.default_font, "{name}: \\deff");
        assert_eq!(again.generator, doc.generator, "{name}: generator");
        // And once more, to catch a writer that is only idempotent by accident.
        assert_eq!(again.to_rtf(), rewritten, "{name}: not idempotent");
    }
}

#[test]
fn every_fixture_that_parses_can_be_written_and_reparsed() {
    let mut checked = 0;
    for entry in fs::read_dir(CORPUS).expect("corpus directory") {
        let path = entry.expect("dir entry").path();
        if path.extension().map(|e| e != "bin").unwrap_or(true) {
            continue;
        }
        let Ok(doc) = Document::parse(&fs::read(&path).expect("fixture")) else {
            continue; // the deliberately malformed ones
        };
        let rewritten = doc.to_rtf();
        let again = Document::parse(&rewritten)
            .unwrap_or_else(|e| panic!("{}: rewritten output does not parse: {e}", path.display()));
        assert_eq!(again.text, doc.text, "{}", path.display());
        checked += 1;
    }
    assert!(
        checked >= 8,
        "expected most fixtures to round-trip, got {checked}"
    );
}

// --------------------------------------------------- the four writer traps

#[test]
fn no_output_byte_is_ever_outside_ascii() {
    // The rule the crate README states, and the reason it states it: a `\'hh`
    // escape or a raw high byte only decodes correctly under the code page the
    // document declares, and the reader on the other end may be under a
    // different one. Then a raw byte arrives as a *different character* rather
    // than as a visible gap.
    let mut w = Writer::new();
    w.push(
        "caf\u{e9} \u{2014} \u{1f600} \u{4e2d}\u{6587} \u{5d0}",
        &WriteProps::default(),
    );
    w.push(
        "\u{2603}",
        &WriteProps {
            font: Some("Wingdings \u{2603}"),
            ..WriteProps::default()
        },
    );
    let bytes = w.finish();
    assert!(
        bytes.is_ascii(),
        "high byte in the output: {:?}",
        String::from_utf8_lossy(&bytes)
    );
    let doc = Document::parse(&bytes).expect("parses");
    assert_eq!(
        doc.text,
        "caf\u{e9} \u{2014} \u{1f600} \u{4e2d}\u{6587} \u{5d0}\u{2603}"
    );
    assert_eq!(doc.fonts[1].name, "Wingdings \u{2603}");
}

#[test]
fn the_declared_uc_count_matches_every_fallback() {
    // `\uc1` in the header and exactly one fallback character per `\uN`. A
    // writer that declares `\uc1` and emits two silently eats a character of
    // real text in every reader that honours the counter, which is most of
    // them.
    let bytes = write([("a\u{2014}b\u{2026}c", WriteProps::default())]);
    let text = String::from_utf8(bytes.clone()).expect("ASCII");
    assert!(
        text.contains(r"\uc1"),
        "the header must state the count: {text}"
    );
    assert!(
        text.contains(r"\u8212-"),
        "em dash with a one-character fallback: {text}"
    );
    assert!(
        text.contains(r"\u8230?"),
        "ellipsis falls back to one char, not three: {text}"
    );
    assert_eq!(Document::parse(&bytes).unwrap().text, "a\u{2014}b\u{2026}c");
}

#[test]
fn a_non_bmp_character_is_two_escapes_and_two_fallbacks() {
    // The spec's `\uN` parameter is a UTF-16 code unit, so U+1F600 is a
    // surrogate pair: two escapes, and under `\uc1` two fallback characters for
    // one character of text. That is what Word does and what the counter means.
    let bytes = write([("\u{1f600}", WriteProps::default())]);
    let text = String::from_utf8(bytes.clone()).expect("ASCII");
    assert!(text.contains(r"\u-10179?\u-8704?"), "{text}");
    assert_eq!(Document::parse(&bytes).unwrap().text, "\u{1f600}");
}

#[test]
fn font_sizes_are_half_points() {
    assert_eq!(half_points(12.0), 24);
    assert_eq!(half_points(18.0), 36);
    assert_eq!(
        half_points(10.5),
        21,
        "half-points can express a half point"
    );
    // Not a size an `\fsN` parameter can hold: the RTF default rather than a
    // clamp that would silently make text microscopic or enormous.
    assert_eq!(half_points(0.0), 24);
    assert_eq!(half_points(-3.0), 24);
    assert_eq!(half_points(f32::NAN), 24);
    assert_eq!(half_points(f32::INFINITY), 24);

    let bytes = write([(
        "x",
        WriteProps {
            size_half_points: Some(half_points(12.0)),
            ..WriteProps::default()
        },
    )]);
    let text = String::from_utf8(bytes).expect("ASCII");
    assert!(
        text.contains(r"\fs24 "),
        "12pt must be \\fs24, not \\fs12: {text}"
    );
}

#[test]
fn the_first_colortbl_entry_is_the_empty_auto_one() {
    let bytes = write([(
        "x",
        WriteProps {
            foreground: Some(Color::new(1, 2, 3)),
            ..WriteProps::default()
        },
    )]);
    let text = String::from_utf8(bytes.clone()).expect("ASCII");
    assert!(
        text.contains(r"{\colortbl;\red1\green2\blue3;}"),
        "the leading `;` is the auto entry `\\cf0` names; dropping it shifts \
         every index by one and recolours the document: {text}"
    );
    let doc = Document::parse(&bytes).unwrap();
    assert_eq!(doc.colors, [None, Some(Color::new(1, 2, 3))]);
    assert_eq!(
        doc.runs[0].props.foreground,
        Some(1),
        "the run points past auto"
    );
}

#[test]
fn a_document_with_no_colours_has_no_colour_table() {
    let bytes = write([("x", WriteProps::default())]);
    let text = String::from_utf8(bytes).expect("ASCII");
    assert!(!text.contains(r"\colortbl"), "nothing to declare: {text}");
    assert!(!text.contains(r"\cf"), "and nothing to reference: {text}");
}

// ------------------------------------------------------------ escaping

#[test]
fn the_three_metacharacters_are_escaped() {
    let bytes = write([(r"a{b}c\d", WriteProps::default())]);
    let text = String::from_utf8(bytes.clone()).expect("ASCII");
    assert!(text.contains(r"a\{b\}c\\d"), "{text}");
    assert_eq!(Document::parse(&bytes).unwrap().text, r"a{b}c\d");
}

#[test]
fn text_that_starts_with_a_space_keeps_it() {
    // Exactly one space after a control word is the delimiter and is eaten, so
    // a run whose text begins with a space needs the second one. Getting this
    // wrong deletes a space at the head of every styled run.
    let bytes = write([(
        " leading",
        WriteProps {
            bold: true,
            ..WriteProps::default()
        },
    )]);
    assert_eq!(Document::parse(&bytes).unwrap().text, " leading");
}

#[test]
fn text_that_starts_with_a_digit_does_not_extend_the_control_word() {
    // `\fs24` followed by the text `5` must not read as `\fs245`.
    let bytes = write([("5 apples", WriteProps::default())]);
    let doc = Document::parse(&bytes).unwrap();
    assert_eq!(doc.text, "5 apples");
    assert_eq!(doc.runs[0].props.size_half_points, 24);
}

#[test]
fn breaks_become_par_and_crlf_is_one_break_not_two() {
    let bytes = write([("a\nb\r\nc\rd\te", WriteProps::default())]);
    assert_eq!(Document::parse(&bytes).unwrap().text, "a\nb\nc\nd\te");
}

#[test]
fn a_font_name_can_carry_a_semicolon() {
    // `;` terminates a `\fonttbl` entry, so a name containing one has to leave
    // as an escape or it splits the entry in half and renames the font.
    let bytes = write([(
        "x",
        WriteProps {
            font: Some("Ugly; Font"),
            ..WriteProps::default()
        },
    )]);
    let doc = Document::parse(&bytes).expect("parses");
    assert_eq!(doc.fonts.len(), 2, "auto plus one, not three");
    assert_eq!(doc.fonts[1].name, "Ugly; Font");
}

#[test]
fn a_font_name_can_carry_braces_and_a_backslash() {
    let bytes = write([(
        "x",
        WriteProps {
            font: Some(r"{Br\ace}"),
            ..WriteProps::default()
        },
    )]);
    let doc = Document::parse(&bytes).expect("parses");
    assert_eq!(doc.fonts[1].name, r"{Br\ace}");
}

// ------------------------------------------------------------ the tables

#[test]
fn identical_formatting_merges_into_one_run() {
    // `rclip-rtf`'s own parser cuts a run at every group boundary and every
    // escape, so a caller replaying its output pushes one piece per character.
    // A writer that emitted a `\plain` preamble for each of them would produce
    // output several times the size of its input for no visible difference.
    let mut w = Writer::new();
    for c in "hello".chars() {
        let mut buf = [0u8; 4];
        w.push(c.encode_utf8(&mut buf), &WriteProps::default());
    }
    let bytes = w.finish();
    let text = String::from_utf8(bytes.clone()).expect("ASCII");
    assert_eq!(text.matches(r"\plain").count(), 1, "{text}");
    assert_eq!(Document::parse(&bytes).unwrap().runs.len(), 1);
}

#[test]
fn a_repeated_font_or_colour_is_interned_once() {
    let red = Some(Color::new(255, 0, 0));
    let bytes = write([
        (
            "a",
            WriteProps {
                font: Some("Georgia"),
                foreground: red,
                ..WriteProps::default()
            },
        ),
        (
            "b",
            WriteProps {
                bold: true,
                ..WriteProps::default()
            },
        ),
        (
            "c",
            WriteProps {
                font: Some("Georgia"),
                foreground: red,
                ..WriteProps::default()
            },
        ),
    ]);
    let doc = Document::parse(&bytes).unwrap();
    assert_eq!(
        doc.fonts.len(),
        2,
        "the default entry and Georgia: {:?}",
        doc.fonts
    );
    assert_eq!(doc.colors.len(), 2, "auto and red: {:?}", doc.colors);
    assert_eq!(doc.runs[0].props.font, doc.runs[2].props.font);
}

#[test]
fn an_unnamed_font_stays_unnamed() {
    // `WriteProps::font: None` means "whatever the reader uses for body text".
    // `\f0`'s name is deliberately empty rather than a real font, because
    // naming one would invent a statement the caller never made — and it would
    // come back as one on the next parse.
    let bytes = write([("x", WriteProps::default())]);
    let doc = Document::parse(&bytes).unwrap();
    assert_eq!(doc.default_font, Some(0));
    assert_eq!(doc.font(0).map(|f| f.name.as_str()), Some(""));
}

#[test]
fn an_empty_document_is_still_a_document() {
    let bytes = Writer::new().finish();
    let doc = Document::parse(&bytes).expect("an empty document must still parse");
    assert_eq!(doc.text, "");
    assert!(doc.runs.is_empty());
}

#[test]
fn empty_pushes_are_ignored() {
    let mut w = Writer::new();
    w.push(
        "",
        &WriteProps {
            bold: true,
            ..WriteProps::default()
        },
    );
    assert!(w.is_empty());
    w.push("x", &WriteProps::default());
    w.push("", &WriteProps::default());
    assert_eq!(Document::parse(&w.finish()).unwrap().text, "x");
}

#[test]
fn a_generator_is_written_only_when_asked_for() {
    let plain = Writer::new();
    assert!(!String::from_utf8(plain.finish())
        .unwrap()
        .contains("generator"));

    let mut w = Writer::new().generator("rich-clipboard test");
    w.push("x", &WriteProps::default());
    let doc = Document::parse(&w.finish()).unwrap();
    assert_eq!(doc.generator.as_deref(), Some("rich-clipboard test"));
    assert_eq!(doc.text, "x", "the destination is not body text");
}

#[test]
fn a_hand_built_document_with_a_gap_writes_the_gap() {
    // `Document::parse` never leaves text uncovered, but `Document`'s fields
    // are public and a caller can. Dropping the uncovered text silently would
    // be the worst of the three options.
    let doc = Document {
        text: "abcdef".into(),
        runs: vec![Run {
            range: 2..4,
            props: CharProps {
                bold: true,
                ..CharProps::DEFAULT
            },
        }],
        ..Document::default()
    };
    let again = Document::parse(&doc.to_rtf()).expect("parses");
    assert_eq!(again.text, "abcdef");
    assert_eq!(again.runs.len(), 3);
    assert!(again.runs[1].props.bold);
    assert!(!again.runs[0].props.bold && !again.runs[2].props.bold);
}

#[test]
fn a_hand_built_document_with_a_backwards_range_does_not_panic() {
    let doc = Document {
        text: "abc".into(),
        runs: vec![
            // Written out rather than as `2..1`, which is a compile-time
            // lint: a backwards range is the point of this test.
            Run {
                range: std::ops::Range { start: 2, end: 1 },
                props: CharProps::DEFAULT,
            },
            Run {
                range: 0..99,
                props: CharProps::DEFAULT,
            },
        ],
        ..Document::default()
    };
    let _ = Document::parse(&doc.to_rtf());
}

#[test]
fn the_font_family_hint_survives_a_document_round_trip() {
    let bytes = fixture("font-color-table.bin");
    let doc = Document::parse(&bytes).unwrap();
    assert!(
        doc.fonts.iter().any(|f| f.family != FontFamily::Nil),
        "{:?}",
        doc.fonts
    );
    let again = Document::parse(&doc.to_rtf()).unwrap();
    assert_eq!(again.fonts, doc.fonts);
}

// -------------------------------------------------------------- fixtures

#[test]
fn the_written_fixtures_match_the_writer() {
    // These are checked into the corpus so a change in the writer's output is
    // visible in a diff rather than only inside an assertion.
    assert_eq!(
        write(styled()),
        fixture("written-styled-runs.bin"),
        "the writer's output changed; regenerate corpus/synthetic/rclip-rtf/written-styled-runs.bin"
    );

    let mut w = Writer::new();
    w.push(
        "caf\u{e9} \u{2014} \u{1f600}\ttab\nbreak {braces} \\slash",
        &WriteProps {
            italic: true,
            ..WriteProps::default()
        },
    );
    assert_eq!(
        w.finish(),
        fixture("written-escapes.bin"),
        "the writer's escaping changed; regenerate corpus/synthetic/rclip-rtf/written-escapes.bin"
    );
}

#[test]
fn a_space_fallback_is_not_eaten_as_the_parameter_delimiter() {
    // Found by the `rtf_write_round_trip` fuzz target.
    //
    // U+00A0's ASCII fallback is a space. Written naively that yields
    // `\u160 `, where the tokenizer consumes the space as the control word's
    // *parameter delimiter* — so the fallback never reaches the `\ucN` counter,
    // which then skips whatever comes next instead. With U+2013 following, the
    // thing skipped is the entire en dash.
    //
    // This is the `\ucN` failure the crate exists to get right, appearing in
    // its own writer.
    let text = "a\u{00A0}\u{2013}b";
    let rtf = write([(text, WriteProps::default())]);
    let back = Document::parse(&rtf).expect("re-parses");
    assert_eq!(
        back.text, text,
        "the en dash after a space-fallback escape must survive"
    );
}

#[test]
fn every_ascii_fallback_round_trips_after_a_non_bmp_neighbour() {
    // The general form of the bug above: any fallback that is itself a
    // delimiter-looking byte has to be written where the counter can see it.
    for c in [
        '\u{00A0}', '\u{2013}', '\u{2014}', '\u{2018}', '\u{201C}', '\u{00AD}',
    ] {
        let text = format!("x{c}\u{2013}y");
        let rtf = write([(text.as_str(), WriteProps::default())]);
        let back = Document::parse(&rtf).expect("re-parses");
        assert_eq!(back.text, text, "U+{:04X} did not round-trip", c as u32);
    }
}

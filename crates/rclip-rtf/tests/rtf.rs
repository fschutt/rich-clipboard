//! Integration tests for `rclip-rtf`.
//!
//! The `\ucN` skip counter, the ignorable-destination rule and the depth bound
//! get the most attention here, because those are the three places where an RTF
//! reader silently produces wrong output instead of failing.

use std::fs;

use rclip_rtf::{
    colors, fonts, generator, header, is_rtf, Codepage, Color, ControlSymbol, Document, ErrorKind,
    FontFamily, Parser, RunText, Token, Tokenizer,
};

// ---------------------------------------------------------------- helpers

const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/synthetic/rclip-rtf/"
);

fn fixture(name: &str) -> Vec<u8> {
    fs::read(format!("{CORPUS}{name}")).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

/// Flatten a document to plain text through the borrowing API.
fn text_of(src: &[u8]) -> String {
    let mut out = String::new();
    for run in Parser::new(src).expect("signature should be accepted") {
        match run.expect("document should parse").text {
            RunText::Text(t) => out.push_str(t),
            RunText::Char(c) => out.push(c),
            RunText::ParagraphBreak | RunText::LineBreak => out.push('\n'),
        }
    }
    out
}

fn err_of(src: &[u8]) -> ErrorKind {
    let kind = Parser::new(src).err().map(|e| e.kind);
    if let Some(k) = kind {
        return k;
    }
    Parser::new(src)
        .unwrap()
        .find_map(|r| r.err())
        .unwrap_or_else(|| panic!("expected a parse error, got a clean parse"))
        .kind
}

fn tokens(src: &[u8]) -> Vec<Token<'_>> {
    Tokenizer::new(src)
        .map(|t| t.expect("should tokenize"))
        .collect()
}

// ---------------------------------------------------------------- tokenizer

#[test]
fn control_word_forms() {
    // `\b0` is bold-off, not `\b` followed by the digit 0; `\fs-24` carries a
    // negative parameter; the single delimiting space belongs to the keyword.
    assert_eq!(
        tokens(br"\b\b0\fs-24\fs24 x"),
        vec![
            Token::ControlWord {
                name: "b",
                param: None
            },
            Token::ControlWord {
                name: "b",
                param: Some(0)
            },
            Token::ControlWord {
                name: "fs",
                param: Some(-24)
            },
            Token::ControlWord {
                name: "fs",
                param: Some(24)
            },
            Token::Text("x"),
        ],
        "control-word shapes"
    );
}

#[test]
fn only_one_delimiting_space_is_eaten() {
    // The spec eats exactly one space. The second is document text, and a
    // tokenizer that eats runs of whitespace loses real indentation.
    assert_eq!(
        tokens(br"\b  x"),
        vec![
            Token::ControlWord {
                name: "b",
                param: None
            },
            Token::Text(" x")
        ],
        "second space must survive as text"
    );
    assert_eq!(
        tokens(b"\\b\tx"),
        vec![
            Token::ControlWord {
                name: "b",
                param: None
            },
            Token::Text("\tx")
        ],
        "a tab is not a keyword delimiter"
    );
}

#[test]
fn control_symbols() {
    assert_eq!(
        tokens(br"\\\{\}\*\~\-\_\'41\|"),
        vec![
            Token::ControlSymbol(ControlSymbol::Literal('\\')),
            Token::ControlSymbol(ControlSymbol::Literal('{')),
            Token::ControlSymbol(ControlSymbol::Literal('}')),
            Token::ControlSymbol(ControlSymbol::Ignorable),
            Token::ControlSymbol(ControlSymbol::NonBreakingSpace),
            Token::ControlSymbol(ControlSymbol::OptionalHyphen),
            Token::ControlSymbol(ControlSymbol::NonBreakingHyphen),
            Token::ControlSymbol(ControlSymbol::HexByte(0x41)),
            Token::ControlSymbol(ControlSymbol::Other(b'|')),
        ],
        "control symbols take no delimiter"
    );
}

#[test]
fn escaped_braces_are_text_not_structure() {
    // `\{` must not open a group. Getting this wrong unbalances the stack on
    // any document that quotes a brace.
    let src = br"{a\{b\}c}";
    assert_eq!(
        tokens(src),
        vec![
            Token::GroupStart,
            Token::Text("a"),
            Token::ControlSymbol(ControlSymbol::Literal('{')),
            Token::Text("b"),
            Token::ControlSymbol(ControlSymbol::Literal('}')),
            Token::Text("c"),
            Token::GroupEnd,
        ],
        "escaped braces stay text"
    );
}

#[test]
fn stream_newlines_and_nul_are_dropped() {
    // RTF writers hard-wrap wherever they like, and `CF_RTF` arrives off the
    // Windows clipboard NUL-terminated. Neither is content -- and neither may
    // reach the `\ucN` skip counter, where it would eat a real character.
    assert_eq!(
        tokens(b"a\r\nb\0c"),
        vec![Token::Text("a"), Token::Text("b"), Token::Text("c")],
        "CR, LF and NUL are not content"
    );
}

#[test]
fn backslash_newline_is_a_paragraph_mark() {
    assert_eq!(
        tokens(b"a\\\nb"),
        vec![
            Token::Text("a"),
            Token::ControlSymbol(ControlSymbol::EmbeddedParagraph),
            Token::Text("b"),
        ],
        "a backslash before a newline means \\par"
    );
}

#[test]
fn unescaped_high_byte_is_a_codepage_byte() {
    assert_eq!(
        tokens(b"a\xe9b"),
        vec![Token::Text("a"), Token::RawByte(0xE9), Token::Text("b")],
        "RTF is a 7-bit format; a high byte means the same as \\'hh"
    );
}

#[test]
fn bin_payload_is_consumed_by_the_lexer() {
    // The payload is arbitrary bytes and routinely contains braces. A lexer
    // that leaves it in the stream hands the parser braces that are not braces.
    let src = br"\bin5 }{}{}rest";
    assert_eq!(
        tokens(src),
        vec![
            Token::ControlWord {
                name: "bin",
                param: Some(5)
            },
            Token::Binary(b"}{}{}"),
            Token::Text("rest"),
        ],
        "\\binN owns the next N bytes whatever they look like"
    );
}

#[test]
fn bin_length_past_the_end_is_an_error_not_a_panic() {
    // The check happens where the length is read, not where the payload is
    // handed back, so the error surfaces on the `\bin` control word itself.
    assert_eq!(
        Tokenizer::new(br"\bin9999 short")
            .next()
            .unwrap()
            .unwrap_err()
            .kind,
        ErrorKind::TooLarge,
        "a length field off the wire must be checked before it is trusted"
    );
}

#[test]
fn truncated_hex_escape_is_malformed() {
    assert_eq!(
        Tokenizer::new(br"\'zz").next().unwrap().unwrap_err().kind,
        ErrorKind::Malformed,
        "\\' with non-hex behind it cannot be resynchronised"
    );
    assert_eq!(
        Tokenizer::new(br"\'4").next().unwrap().unwrap_err().kind,
        ErrorKind::UnexpectedEof,
        "\\' with one digit ran off the end"
    );
}

#[test]
fn trailing_backslash_is_eof() {
    assert_eq!(
        Tokenizer::new(b"abc\\").nth(1).unwrap().unwrap_err().kind,
        ErrorKind::UnexpectedEof,
        "the stream was cut mid-escape"
    );
}

#[test]
fn overlong_control_word_is_not_rejected() {
    // The spec caps control words at 32 letters, but a longer one is by
    // definition unknown, so it fails lookup on its own. Rejecting the document
    // over it would throw away content for no safety gain.
    let name = "a".repeat(200);
    let src = format!("\\{name} text");
    let toks = tokens(src.as_bytes());
    assert_eq!(toks.len(), 2, "one control word plus its text");
    assert!(matches!(toks[0], Token::ControlWord { param: None, .. }));
    assert_eq!(toks[1], Token::Text("text"));
}

#[test]
fn absurd_numeric_parameter_saturates() {
    assert_eq!(
        tokens(br"\fs999999999999999999999"),
        vec![Token::ControlWord {
            name: "fs",
            param: Some(i32::MAX)
        }],
        "junk in one property is not a reason to drop the document"
    );
}

#[test]
fn lone_minus_is_not_a_parameter() {
    assert_eq!(
        tokens(br"\b-x"),
        vec![
            Token::ControlWord {
                name: "b",
                param: None
            },
            Token::Text("-x")
        ],
        "a hyphen with no digits behind it is text"
    );
}

// ------------------------------------------------------- \uc skip counter

#[test]
fn unicode_skip_counter() {
    // The whole fixture in one assertion: \uc2, a nested \uc1 restored on `}`,
    // a negative \uN, \'hh fallbacks counted one character each, and a `}` that
    // truncates a skip.
    assert_eq!(
        text_of(&fixture("unicode-uc2.bin")),
        "A\u{20AC}BC\u{2014}DE\u{EFCF}FG\u{2603}HI\u{2603}J\n",
        "\\ucN skip counting across groups"
    );
}

#[test]
fn uc_is_restored_on_group_end() {
    // Outer \uc2, inner \uc1. If \uc were global, the `Z` after the group would
    // be eaten as a leftover fallback character.
    let src = br"{\rtf1\ansi\uc2 {\uc1 \u9731 x}\u9731 yyZ}";
    assert_eq!(
        text_of(src),
        "\u{2603}\u{2603}Z",
        "the inner \\uc1 must not leak out"
    );

    // And the other direction: a larger inner count must not survive either.
    let src = br"{\rtf1\ansi\uc1 {\uc2 \u9731 xx}\u9731 yZ}";
    assert_eq!(
        text_of(src),
        "\u{2603}\u{2603}Z",
        "the inner \\uc2 must not leak out"
    );
}

#[test]
fn hex_escape_counts_as_one_skippable_character() {
    // Two bytes of source, one character of skip. A parser that counts bytes
    // eats the following text.
    let src = br"{\rtf1\ansi\uc2 A\u9731\'3f\'3fB}";
    assert_eq!(
        text_of(src),
        "A\u{2603}B",
        "\\'hh is one character, not two bytes"
    );
}

#[test]
fn control_word_counts_as_one_skippable_character() {
    let src = br"{\rtf1\ansi\uc1 A\u9731\tab B}";
    assert_eq!(
        text_of(src),
        "A\u{2603}B",
        "the \\tab is the fallback and must not become a tab in the output"
    );
}

#[test]
fn brace_ends_a_skip_early() {
    // Spec: "If an RTF scope delimiter character is encountered while scanning
    // skippable data, the skippable data is considered to be ended before the
    // delimiter." Without this the `}` is swallowed and the group stack unwinds
    // one level short for the rest of the document.
    let src = br"{\rtf1\ansi\uc2 {A\u9731}B}";
    assert_eq!(
        text_of(src),
        "A\u{2603}B",
        "a brace terminates skippable data"
    );
}

#[test]
fn uc0_skips_nothing() {
    let src = br"{\rtf1\ansi\uc0 A\u9731 B}";
    assert_eq!(
        text_of(src),
        "A\u{2603}B",
        "\\uc0 means there is no fallback to skip"
    );
}

#[test]
fn bin_and_its_payload_count_as_one_skippable_character() {
    // Spec: "a \bin keyword, its argument, and the binary data that follows are
    // considered one character for skipping purposes."
    let src = br"{\rtf1\ansi\uc1 A\u9731\bin3 xyzB}";
    assert_eq!(
        text_of(src),
        "A\u{2603}B",
        "\\bin plus payload is one skippable character"
    );
}

#[test]
fn negative_unicode_wraps_around() {
    // Spec: values above 32767 are written as negative numbers, so \u-4145 is
    // code unit 61391.
    let src = br"{\rtf1\ansi\uc0 \u-4145}";
    assert_eq!(text_of(src), "\u{EFCF}", "signed 16-bit wraparound");
}

#[test]
fn unsigned_and_scalar_unicode_are_accepted_too() {
    // Not spec-legal, but writers that are not Word emit both forms, and a
    // reader that rejects them loses real characters for nothing.
    assert_eq!(
        text_of(br"{\rtf1\ansi\uc0 \u61391}"),
        "\u{EFCF}",
        "unsigned 0..65535"
    );
    assert_eq!(
        text_of(br"{\rtf1\ansi\uc0 \u128512}"),
        "\u{1F600}",
        "full scalar value"
    );
}

#[test]
fn surrogate_pair_is_joined() {
    assert_eq!(
        text_of(&fixture("surrogate-pair.bin")),
        "hi \u{1F60A} there\n",
        "two consecutive \\uN halves make one character"
    );
}

#[test]
fn lone_surrogate_becomes_replacement() {
    assert_eq!(
        text_of(&fixture("lone-surrogate.bin")),
        "a\u{FFFD}b\n",
        "a truncated pair must not panic and must not emit a lone surrogate"
    );
}

#[test]
fn lone_low_surrogate_becomes_replacement() {
    assert_eq!(
        text_of(br"{\rtf1\ansi\uc0 a\u56842 b}"),
        "a\u{FFFD}b",
        "unpaired low surrogate"
    );
}

// ------------------------------------------------------- destination rules

#[test]
fn ignorable_destination_is_skipped_wholesale() {
    assert_eq!(
        text_of(&fixture("nested-unknown-dest.bin")),
        "Visible  more kept end\n",
        "the starred destination and its nested group go away; the unstarred one does not"
    );
}

#[test]
fn unknown_control_word_without_star_keeps_its_text() {
    // The other half of the rule. This is why the visible half of a hyperlink,
    // `{\fldrslt text}`, survives.
    assert_eq!(
        text_of(br"{\rtf1\ansi {\mystery kept text}}"),
        "kept text",
        "an unmarked unknown destination is not a reason to drop text"
    );
}

#[test]
fn field_result_survives_and_field_instruction_does_not() {
    let src = br#"{\rtf1\ansi{\field{\*\fldinst HYPERLINK "http://x/"}{\fldrslt click me}}}"#;
    assert_eq!(
        text_of(src),
        "click me",
        "the rendered half of a field is body text"
    );
}

#[test]
fn header_table_text_is_not_body_text() {
    // `{\fonttbl ...}` carries no `\*`. A reader that treats the unknown
    // `\fonttbl` as "ignore the word, keep the text" pastes `Helvetica;` into
    // the user's document.
    let body = text_of(&fixture("minimal.bin"));
    assert_eq!(body, "Hello, world!\n", "only the body survives");
    assert!(!body.contains("Helvetica"), "font names are not body text");
}

#[test]
fn pict_hex_data_is_not_body_text() {
    // `\pict` is not `\*`-marked in older RTF and its body is megabytes of hex.
    let src = br"{\rtf1\ansi before{\pict\wmetafile8 0102030405060708}after}";
    assert_eq!(text_of(src), "beforeafter", "picture data is not text");
}

#[test]
fn list_bullet_text_is_dropped() {
    // Word emits `{\pntext\f2\'b7\tab}` ahead of a list paragraph for readers
    // that do not understand `\pn`; keeping it litters the paste with bullets.
    let src = b"{\\rtf1\\ansi{\\pntext\\f2\\'b7\\tab}Item}";
    assert_eq!(text_of(src), "Item", "the legacy bullet is not content");
}

// --------------------------------------------------------- character props

#[test]
fn character_properties_restore_on_group_end() {
    let src = br"{\rtf1\ansi a{\b\i b}c}";
    let runs: Vec<_> = Parser::new(src).unwrap().map(|r| r.unwrap()).collect();
    let styled: Vec<(&str, bool, bool)> = runs
        .iter()
        .filter_map(|r| match r.text {
            RunText::Text(t) => Some((t, r.props.bold, r.props.italic)),
            _ => None,
        })
        .collect();
    assert_eq!(
        styled,
        vec![("a", false, false), ("b", true, true), ("c", false, false)],
        "properties are per group and restored on the closing brace"
    );
}

#[test]
fn plain_resets_every_character_property() {
    let src = br"{\rtf1\ansi\b\i\ul\strike\fs48\cf2 x\plain y}";
    let runs: Vec<_> = Parser::new(src).unwrap().map(|r| r.unwrap()).collect();
    let x = runs
        .iter()
        .find(|r| r.text == RunText::Text("x"))
        .expect("run x");
    let y = runs
        .iter()
        .find(|r| r.text == RunText::Text("y"))
        .expect("run y");
    assert!(x.props.bold && x.props.italic && x.props.underline && x.props.strike);
    assert_eq!(x.props.size_half_points, 48);
    assert_eq!(x.props.foreground, Some(2));
    assert!(
        y.props.is_plain(),
        "\\plain resets everything, colour and size included"
    );
    assert_eq!(
        y.props.size_half_points, 24,
        "the RTF default is 24 half-points, i.e. 12pt"
    );
}

#[test]
fn font_size_is_half_points() {
    let src = br"{\rtf1\ansi\fs28 x}";
    let run = Parser::new(src).unwrap().next().unwrap().unwrap();
    assert_eq!(run.props.size_half_points, 28);
    assert_eq!(run.props.points(), 14.0, "\\fs28 is 14pt, not 28pt");
}

#[test]
fn underline_variants_and_ulnone() {
    assert!(
        Parser::new(br"{\rtf1\ansi\ulwave x}")
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .props
            .underline,
        "every \\ul* variant is some kind of underline"
    );
    let src = br"{\rtf1\ansi\ul a\ulnone b}";
    let runs: Vec<_> = Parser::new(src).unwrap().map(|r| r.unwrap()).collect();
    assert!(runs[0].props.underline);
    assert!(!runs[1].props.underline, "\\ulnone turns it off");
}

#[test]
fn breaks_and_tab() {
    let src = br"{\rtf1\ansi a\line b\tab c\par d}";
    let kinds: Vec<_> = Parser::new(src).unwrap().map(|r| r.unwrap().text).collect();
    assert_eq!(
        kinds,
        vec![
            RunText::Text("a"),
            RunText::LineBreak,
            RunText::Text("b"),
            RunText::Char('\t'),
            RunText::Text("c"),
            RunText::ParagraphBreak,
            RunText::Text("d"),
        ],
        "\\line and \\par stay distinguishable in the borrowing API"
    );
}

#[test]
fn named_symbol_control_words() {
    // Word emits these instead of `\uN` for punctuation that Windows-1252 can
    // represent; dropping them turns "don't" into "dont".
    assert_eq!(
        text_of(br"{\rtf1\ansi don\rquote t \endash \emdash \bullet}"),
        // Each keyword swallows its own delimiting space, so only the one
        // typed after `t` survives.
        "don\u{2019}t \u{2013}\u{2014}\u{2022}",
        "named punctuation keywords"
    );
}

// ------------------------------------------------------------- code pages

#[test]
fn hex_escapes_decode_through_windows_1252() {
    assert_eq!(
        text_of(&fixture("hex-escapes-cp1252.bin")),
        "caf\u{E9} \u{201C}quoted\u{201D} \u{20AC}\n",
        "0x80..0x9F is where Windows-1252 differs from Latin-1"
    );
}

#[test]
fn unsupported_codepage_is_lossy_not_wrong() {
    // A wrong guess produces mojibake that looks like text and survives into
    // the user's document; U+FFFD is at least visibly a gap.
    let src = b"{\\rtf1\\mac\\ansicpg10000 caf\\'8e}";
    assert_eq!(text_of(src), "caf\u{FFFD}");
    assert_eq!(header(src).unwrap().codepage, Codepage::Unsupported(10000));
}

#[test]
fn latin1_and_1252_differ_where_they_should() {
    assert_eq!(
        text_of(b"{\\rtf1\\ansi\\ansicpg28591 \\'80}"),
        "\u{80}",
        "Latin-1 is transparent"
    );
    assert_eq!(
        text_of(b"{\\rtf1\\ansi\\ansicpg1252 \\'80}"),
        "\u{20AC}",
        "1252 maps 0x80 to euro"
    );
}

// ------------------------------------------------------------- the tables

#[test]
fn font_and_color_tables() {
    let src = fixture("font-color-table.bin");
    let head = header(&src).unwrap();
    assert_eq!(head.codepage, Codepage::Windows1252);
    assert_eq!(head.default_font, Some(0));

    let cols: Vec<_> = colors(&src).collect();
    assert_eq!(
        cols,
        vec![
            None,
            Some(Color::new(255, 0, 0)),
            Some(Color::new(0, 0, 255))
        ],
        "the first entry is the conventional empty 'auto' colour and must stay None"
    );

    let fs: Vec<_> = fonts(&src, head.codepage).collect();
    let described: Vec<_> = fs
        .iter()
        .map(|f| {
            (
                f.id,
                f.family,
                f.charset,
                f.name.as_str().expect("ASCII name"),
            )
        })
        .collect();
    assert_eq!(
        described,
        vec![
            (0, FontFamily::Swiss, Some(0), "Helvetica"),
            (1, FontFamily::Modern, Some(0), "Courier New"),
            // The nested `{\*\panose ...}` must not end up in the name.
            (2, FontFamily::Roman, Some(0), "Times New Roman"),
        ],
        "font table entries"
    );

    let gen = generator(&src, head.codepage).expect("generator destination");
    assert_eq!(gen.chars().collect::<String>(), "Riched20 10.0.19041");

    assert_eq!(text_of(&src), "red blue bold back\n");
}

#[test]
fn flat_font_table_form_is_understood() {
    // Older writers emit the entries without per-font sub-groups.
    let src = br"{\rtf1\ansi{\fonttbl\f0\froman Times;\f1\fmodern Courier;}x}";
    let fs: Vec<_> = fonts(src, Codepage::Windows1252).collect();
    let described: Vec<_> = fs
        .iter()
        .map(|f| (f.id, f.family, f.name.as_str().unwrap()))
        .collect();
    assert_eq!(
        described,
        vec![
            (0, FontFamily::Roman, "Times"),
            (1, FontFamily::Modern, "Courier")
        ]
    );
}

#[test]
fn font_name_with_escapes_still_decodes() {
    // A name that is not a contiguous slice of the input: `as_str` declines and
    // `chars` still works. That is the whole reason RtfText exists.
    let src = b"{\\rtf1\\ansi{\\fonttbl{\\f0\\fnil Caf\\'e9 Sans;}}x}";
    let f = fonts(src, Codepage::Windows1252).next().expect("one font");
    assert!(
        f.name.as_str().is_none(),
        "the name contains an escape, so there is no slice"
    );
    assert_eq!(f.name.chars().collect::<String>(), "Caf\u{E9} Sans");
}

#[test]
fn missing_tables_are_empty_not_errors() {
    let src = br"{\rtf1\ansi hi}";
    assert_eq!(colors(src).count(), 0);
    assert_eq!(fonts(src, Codepage::Windows1252).count(), 0);
    assert!(generator(src, Codepage::Windows1252).is_none());
}

// ------------------------------------------------------------ malformed

#[test]
fn depth_bomb_returns_depth_limit_not_a_stack_overflow() {
    assert_eq!(
        err_of(&fixture("depth-bomb.bin")),
        ErrorKind::DepthLimit,
        "nesting is bounded by rclip_core::MAX_DEPTH with a loop, not recursion"
    );
}

#[test]
fn nesting_up_to_the_limit_is_accepted() {
    // One less than the limit must still work, or the bound is off by one and
    // real documents start failing.
    let depth = rclip_core::MAX_DEPTH as usize - 1;
    let src = format!(
        "{{\\rtf1\\ansi{}ok{}}}",
        "{".repeat(depth),
        "}".repeat(depth)
    );
    assert_eq!(text_of(src.as_bytes()), "ok", "{depth} groups is legal");

    let too_deep = rclip_core::MAX_DEPTH as usize;
    let src = format!(
        "{{\\rtf1\\ansi{}no{}}}",
        "{".repeat(too_deep),
        "}".repeat(too_deep)
    );
    assert_eq!(
        err_of(src.as_bytes()),
        ErrorKind::DepthLimit,
        "one past the limit fails"
    );
}

#[test]
fn unclosed_group_is_eof() {
    assert_eq!(
        err_of(&fixture("unclosed-group.bin")),
        ErrorKind::UnexpectedEof
    );
}

#[test]
fn extra_close_brace_is_malformed() {
    assert_eq!(
        err_of(&fixture("extra-close-brace.bin")),
        ErrorKind::Malformed
    );
}

#[test]
fn non_rtf_payload_is_rejected_by_signature() {
    let src = fixture("not-rtf.bin");
    assert!(!is_rtf(&src));
    assert_eq!(
        err_of(&src),
        ErrorKind::BadMagic,
        "a source that labels HTML as public.rtf is a thing that happens"
    );
}

#[test]
fn bin_payload_does_not_desync_the_group_stack() {
    assert_eq!(
        text_of(&fixture("bin-payload.bin")),
        "AB\n",
        "eleven braces of binary payload are data, not structure"
    );
}

#[test]
fn every_prefix_of_every_fixture_terminates_without_panicking() {
    // Truncation is what a failed clipboard transfer produces, and a parser
    // that panics on it takes the host application down.
    for entry in fs::read_dir(CORPUS).expect("corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let bytes = fs::read(&path).expect("fixture");
        for cut in 0..=bytes.len() {
            let prefix = &bytes[..cut];
            if let Ok(p) = Parser::new(prefix) {
                for run in p {
                    if run.is_err() {
                        break;
                    }
                }
            }
            let _ = Document::parse(prefix);
            let _ = colors(prefix).count();
            let _ = fonts(prefix, Codepage::Windows1252).count();
            let _ = generator(prefix, Codepage::Windows1252);
            for t in Tokenizer::new(prefix) {
                if t.is_err() {
                    break;
                }
            }
        }
    }
}

#[test]
fn every_sidecar_agrees_with_the_parser() {
    // The sidecars are documentation only if nothing checks them.
    let mut checked = 0;
    for entry in fs::read_dir(CORPUS).expect("corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let json = fs::read_to_string(path.with_extension("json"))
            .unwrap_or_else(|e| panic!("sidecar for {}: {e}", path.display()));
        let expect = json_field(&json, "expect").expect("expect field");
        let bytes = fs::read(&path).expect("fixture");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        match expect.as_str() {
            "ok" => {
                Document::parse(&bytes).unwrap_or_else(|e| panic!("{name} should parse: {e}"));
            }
            "error" => {
                let want = json_field(&json, "error_kind")
                    .unwrap_or_else(|| panic!("{name}: error fixtures need an error_kind"));
                let got = Document::parse(&bytes)
                    .err()
                    .unwrap_or_else(|| panic!("{name} should have failed"));
                assert_eq!(got.kind.as_str(), want, "{name} error kind");
            }
            other => panic!("{name}: unknown expect value {other:?}"),
        }
        checked += 1;
    }
    assert!(
        checked >= 10,
        "expected the whole fixture set, saw {checked}"
    );
}

/// Pull one string value out of a sidecar. No value in the corpus contains an
/// escaped quote, which is what keeps this to five lines instead of a
/// dependency.
fn json_field(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = &json[at..];
    let open = rest.find('"')? + 1;
    let rest = &rest[open..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

// ------------------------------------------------------------- Document

#[test]
fn document_merges_adjacent_equal_runs() {
    // The borrowing parser cuts a run at every escape and every group boundary,
    // so an unmerged document has roughly one run per character.
    let src = b"{\\rtf1\\ansi\\ansicpg1252 caf\\'e9 and caf\\'e9\\b bold\\b0 end}";
    let doc = Document::parse(src).unwrap();
    // Every `\b0` eats its own delimiting space, hence "boldend".
    assert_eq!(doc.text, "caf\u{E9} and caf\u{E9}boldend");
    assert_eq!(doc.runs.len(), 3, "unstyled / bold / unstyled");
    // Four source fragments -- text, escape, text, escape -- merged into one.
    assert_eq!(doc.run_text(&doc.runs[0]), "caf\u{E9} and caf\u{E9}");
    assert_eq!(doc.run_text(&doc.runs[1]), "bold");
    assert!(doc.runs[1].props.bold);
    assert_eq!(doc.run_text(&doc.runs[2]), "end");
}

#[test]
fn document_runs_tile_the_text_exactly() {
    let doc = Document::parse(&fixture("font-color-table.bin")).unwrap();
    let mut at = 0;
    for run in &doc.runs {
        assert_eq!(run.range.start, at, "runs must not overlap or leave gaps");
        at = run.range.end;
    }
    assert_eq!(at, doc.text.len(), "runs must cover the whole text");
}

#[test]
fn document_resolves_fonts_and_colors_by_id() {
    let doc = Document::parse(&fixture("font-color-table.bin")).unwrap();
    assert_eq!(doc.text, "red blue bold back\n");
    assert_eq!(doc.generator.as_deref(), Some("Riched20 10.0.19041"));
    assert_eq!(doc.color(0), None, "index 0 is the auto colour");
    assert_eq!(doc.color(1), Some(Color::new(255, 0, 0)));
    assert_eq!(
        doc.font(2).map(|f| f.name.as_str()),
        Some("Times New Roman")
    );
    assert_eq!(
        doc.font(99),
        None,
        "unknown font ids resolve to nothing, not to a panic"
    );

    let first = &doc.runs[0];
    assert_eq!(doc.run_text(first), "red ");
    assert_eq!(first.props.foreground, Some(1));
    assert_eq!(first.props.size_half_points, 28);
}

#[test]
fn document_parse_propagates_structural_errors() {
    assert_eq!(
        Document::parse(&fixture("depth-bomb.bin"))
            .unwrap_err()
            .kind,
        ErrorKind::DepthLimit
    );
}

#[test]
fn star_only_marks_a_destination_at_the_start_of_a_group() {
    // A literal asterisk in body text needs no escape, so a `\*` anywhere but
    // directly after `{` is malformed input. Honouring it there would silently
    // drop the rest of the group.
    assert_eq!(
        text_of(br"{\rtf1\ansi kept \*\foo also kept}"),
        "kept also kept",
        "a stray \\* must not take the rest of the group with it"
    );
    assert_eq!(
        text_of(br"{\rtf1\ansi kept {\*\foo dropped}}"),
        "kept ",
        "at the start of a group it still marks the destination"
    );
}

#[test]
fn mutated_fixtures_never_panic() {
    // Truncation is one failure mode; corruption is the other. A deterministic
    // LCG over every fixture, flipping one byte at a time to values chosen from
    // the bytes that steer the parser -- braces, backslash, quote, digits.
    const INTERESTING: [u8; 10] = [b'{', b'}', b'\\', b'\'', b'*', b'u', b'c', b'0', 0x00, 0xFF];
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };

    for entry in fs::read_dir(CORPUS).expect("corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let original = fs::read(&path).expect("fixture");
        if original.is_empty() {
            continue;
        }
        for _ in 0..400 {
            let mut bytes = original.clone();
            for _ in 0..3 {
                let at = next() % bytes.len();
                bytes[at] = INTERESTING[next() % INTERESTING.len()];
            }
            // The contract is "returns, without panicking". What it returns on
            // garbage is not specified.
            let _ = Document::parse(&bytes);
            if let Ok(p) = Parser::new(&bytes) {
                for run in p.take(100_000) {
                    if run.is_err() {
                        break;
                    }
                }
            }
            let _ = colors(&bytes).take(10_000).count();
            let _ = fonts(&bytes, Codepage::Windows1252).take(10_000).count();
            let _ = generator(&bytes, Codepage::Windows1252);
        }
    }
}

#[test]
fn table_cells_do_not_run_together() {
    // Tables are not modelled, but emitting nothing at a cell boundary turns
    // "a1" and "b1" into "a1b1".
    let src = br"{\rtf1\ansi\trowd a1\cell b1\cell\row after}";
    assert_eq!(
        text_of(src),
        "a1\tb1\t\nafter",
        "a tab between cells, a break between rows"
    );
}

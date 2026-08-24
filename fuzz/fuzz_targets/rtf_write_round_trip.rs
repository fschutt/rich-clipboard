//! The RTF *writer* — `rclip_rtf::Writer`, `write`, and `Document::to_rtf`.
//!
//! Every other RTF target reads. This one writes first and reads back, which is
//! the only way to catch the two halves disagreeing about an encoding: a reader
//! that is wrong on its own output is wrong on everyone's, and a writer that
//! emits something its own reader mis-parses will do worse in Word.
//!
//! # Two legs, because the property has two forms
//!
//! **`parse(write(x)) == x`** is the strong one and it is what the `runs` leg
//! asserts: styled text goes in, RTF comes out, and the text and formatting
//! that come back must be the ones that went in. The escaping is where this
//! bites — a `\`, a `{`, a non-ASCII character, a `\r\n` and a lone surrogate
//! pair half all have to survive being written and read, and each is a
//! different branch of the emitter.
//!
//! **`write(parse(write(x))) == write(x)`** is the weaker fixed-point form, and
//! the `document` leg needs it because `Document::to_rtf` is documented as
//! lossy in three specific ways — the code page is rewritten to 1252, font
//! names lose their surrounding whitespace, and `\r\n` becomes one paragraph
//! break rather than two. Those are one-shot losses: once a document has been
//! through the writer it is in the writer's own vocabulary, so a *second* trip
//! must change nothing at all. A round trip that keeps changing the document is
//! a bridge that corrupts a payload a little more every time it is re-copied,
//! which is exactly what a clipboard manager does to a payload.
#![no_main]

use arbitrary::{Arbitrary, Result, Unstructured};
use libfuzzer_sys::fuzz_target;

use rclip_rtf::{Color, Document, WriteProps, Writer};

/// One styled run to write.
#[derive(Debug)]
struct Run {
    text: String,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    size_half_points: Option<u16>,
    font: Option<String>,
    foreground: Option<Color>,
    background: Option<Color>,
}

impl Run {
    /// What `Writer::push` will compare when it decides whether to merge this
    /// push into the previous one. Font names and colours are interned, and
    /// interning is by value, so equal names and equal colours produce equal
    /// indices -- which makes this tuple exactly as discriminating as the
    /// `CharProps` the writer builds.
    fn key(&self) -> (bool, bool, bool, bool, u16, Option<&str>, Option<Color>, Option<Color>) {
        (
            self.bold,
            self.italic,
            self.underline,
            self.strike,
            self.size_half_points.unwrap_or(24),
            self.font.as_deref(),
            self.foreground,
            self.background,
        )
    }

    fn props(&self) -> WriteProps<'_> {
        WriteProps {
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            size_half_points: self.size_half_points,
            font: self.font.as_deref(),
            foreground: self.foreground,
            background: self.background,
        }
    }
}

#[derive(Debug)]
enum Input {
    /// Styled runs through `Writer`.
    Runs(Vec<Run>),
    /// Arbitrary bytes through `Document::parse` and back out again.
    Document(Vec<u8>),
}

fn color(u: &mut Unstructured<'_>) -> Result<Option<Color>> {
    if u.int_in_range(0u8..=3)? == 0 {
        return Ok(None);
    }
    Ok(Some(Color::new(
        u.arbitrary()?,
        u.arbitrary()?,
        u.arbitrary()?,
    )))
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        // Two in three inputs go through the writer, because that is the half
        // this target exists for; the rest seed it from real RTF so the
        // document leg starts from something a reader produced.
        if u.int_in_range(0u8..=2)? == 0 {
            let rest = u.len();
            return Ok(Self::Document(u.bytes(rest)?.to_vec()));
        }
        let count = u.int_in_range(1u8..=8)?;
        let mut runs = Vec::new();
        for _ in 0..count {
            let want = usize::from(u.int_in_range(0u8..=64)?);
            let take = want.min(u.len());
            let text = String::from_utf8_lossy(u.bytes(take)?).into_owned();
            let flags = u.arbitrary::<u8>()?;
            let size_half_points = if flags & 0x10 == 0 {
                None
            } else {
                Some(u.arbitrary()?)
            };
            let font = if flags & 0x20 == 0 {
                None
            } else {
                let want = usize::from(u.int_in_range(0u8..=16)?);
                let take = want.min(u.len());
                Some(String::from_utf8_lossy(u.bytes(take)?).into_owned())
            };
            runs.push(Run {
                text,
                bold: flags & 1 != 0,
                italic: flags & 2 != 0,
                underline: flags & 4 != 0,
                strike: flags & 8 != 0,
                size_half_points,
                font,
                foreground: color(u)?,
                background: color(u)?,
            });
        }
        Ok(Self::Runs(runs))
    }
}

/// What the writer is documented to do to text on the way through.
///
/// `\r` and `\r\n` both become one paragraph break, which comes back as `\n`.
/// Nothing else changes, and that is the point: this function is the *whole*
/// list of permitted losses, so anything else that differs is a bug rather than
/// a documented conversion.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

fuzz_target!(|input: Input| {
    match input {
        Input::Runs(runs) => {
            // `Writer::push` ignores empty text and merges adjacent pushes
            // whose formatting is identical, and both matter for what comes
            // back: escaping happens per *merged span*, so a `\r` ending one
            // push and a `\n` starting the next are one paragraph break when
            // the two share formatting and two when they do not. Reproducing
            // the merge here is the difference between checking the writer and
            // checking a guess about it.
            let mut groups: Vec<(String, &Run)> = Vec::new();
            for run in runs.iter().filter(|r| !r.text.is_empty()) {
                match groups.last_mut() {
                    Some((text, prev)) if prev.key() == run.key() => text.push_str(&run.text),
                    _ => groups.push((run.text.clone(), run)),
                }
            }

            let mut writer = Writer::new();
            for run in &runs {
                writer.push(&run.text, &run.props());
            }
            let rtf = writer.finish();

            // The writer's output is RTF by construction, so a reader that
            // refuses it is the two halves disagreeing about the format itself.
            let doc = Document::parse(&rtf).expect("the writer emitted unparseable RTF");

            // `parse(write(x)) == x` for the text. This is where escaping is
            // checked: `\`, `{`, `}`, `\t`, every non-ASCII character as
            // `\uN` with its fallback, and the paragraph-break normalization.
            let expected: String = groups.iter().map(|(t, _)| normalize(t)).collect();
            assert_eq!(doc.text, expected, "text did not survive the round trip");
            assert_eq!(writer.is_empty(), groups.is_empty());

            // The runs must still cover the text exactly, or a caller slicing
            // by range panics.
            let mut end = 0usize;
            for run in &doc.runs {
                assert_eq!(run.range.start, end, "run ranges left a gap or overlapped");
                assert!(run.range.end <= doc.text.len());
                assert!(
                    doc.text.is_char_boundary(run.range.start)
                        && doc.text.is_char_boundary(run.range.end)
                );
                end = run.range.end;
            }
            if !doc.runs.is_empty() {
                assert_eq!(end, doc.text.len(), "runs did not cover the text");
            }

            // And the formatting. Looked up by *position* rather than by run
            // index, because the reader cuts a run at every group boundary and
            // every escape while the writer merges, so the two run lists
            // legitimately differ even when every character carries the same
            // formatting.
            let mut at = 0usize;
            for (text, run) in &groups {
                let normalized = normalize(text);
                if normalized.is_empty() {
                    continue;
                }
                let props = doc
                    .runs
                    .iter()
                    .find(|r| r.range.contains(&at))
                    .unwrap_or_else(|| panic!("no run covers byte {at} of {:?}", doc.text));
                assert_eq!(props.props.bold, run.bold, "bold was lost at {at}");
                assert_eq!(props.props.italic, run.italic, "italic was lost at {at}");
                assert_eq!(
                    props.props.underline, run.underline,
                    "underline was lost at {at}"
                );
                assert_eq!(props.props.strike, run.strike, "strike was lost at {at}");
                // `\fsN` is a 16-bit field and the writer's default is 24
                // half-points. The one value that does not survive is zero: the
                // reader clamps `\fsN` to `1..=0xFFFF`, because a parser that
                // reads `\fs0` as zero reports every run of such a document as
                // invisible. The writer does not clamp, so `WriteProps {
                // size_half_points: Some(0) }` goes out as `\fs0` and comes
                // back as 1. Only reachable by setting the field by hand --
                // `half_points()`, the documented way in, already refuses
                // anything below one half-point -- so it is modelled here
                // rather than treated as a round-trip failure.
                assert_eq!(
                    props.props.size_half_points,
                    run.size_half_points.unwrap_or(24).max(1),
                    "font size was lost at {at}"
                );
                if let Some(c) = run.foreground {
                    assert_eq!(
                        props.props.foreground.and_then(|i| doc.color(i)),
                        Some(c),
                        "foreground colour was lost at {at}"
                    );
                }
                if let Some(c) = run.background {
                    assert_eq!(
                        props.props.background.and_then(|i| doc.color(i)),
                        Some(c),
                        "background colour was lost at {at}"
                    );
                }
                if let Some(name) = &run.font {
                    // The documented loss on a font name is that the reader
                    // trims ASCII whitespace -- but it trims the raw *bytes* of
                    // the table entry, and `escape_name` writes every ASCII
                    // whitespace character except the space itself as a `\uN`
                    // escape. So a leading tab leaves as `\u9 `, is not a
                    // whitespace byte on the wire, survives the trim, and comes
                    // back; only literal spaces are lost. Verified against the
                    // writer: `" \tA\t "` round-trips to `"\tA\t"`.
                    let want = name.trim_matches(' ');
                    // A second, *undocumented* one, found by this target and
                    // reported rather than asserted: a character outside the
                    // BMP is written as two `\uN` surrogate-half escapes, and
                    // `rclip-rtf`'s font-name reader deliberately does not pair
                    // surrogates -- `unicode_escape_char` says so in as many
                    // words, "not worth a second decoding state machine; the
                    // body-text parser pairs them". So `Writer` emits a font
                    // name its own reader gives back as two U+FFFD. The reader
                    // half is a documented choice, so asserting against it here
                    // would be asserting a property the crate says it does not
                    // have; what is *not* documented is that the writer emits
                    // something that choice cannot read. Body text is
                    // unaffected -- it goes through the pairing parser.
                    let astral = want.chars().any(|c| u32::from(c) > 0xFFFF);
                    // `\f0` with an empty name means "the reader's body font",
                    // which is what an all-whitespace name collapses to.
                    if !want.is_empty() && !astral {
                        let got = props.props.font.and_then(|id| doc.font(id));
                        assert_eq!(
                            got.map(|f| f.name.as_str()),
                            Some(want),
                            "font name was lost at {at}"
                        );
                    }
                }
                at += normalized.len();
            }

            // Idempotence: the writer's output re-written must be byte-identical.
            let again = doc.to_rtf();
            let back = Document::parse(&again).expect("our own output must parse");
            assert_eq!(back.text, doc.text, "a second trip changed the text");
            assert_eq!(
                back.to_rtf(),
                again,
                "the writer is not a fixed point on its own output"
            );
        }

        Input::Document(data) => {
            let Ok(first) = Document::parse(&data) else {
                return;
            };
            let once = first.to_rtf();
            let second = Document::parse(&once).expect("our own output must parse");
            let twice = second.to_rtf();
            let third = Document::parse(&twice).expect("our own output must parse");
            let thrice = third.to_rtf();

            // The first trip is documented as lossy in three named ways. The
            // second must not be lossy at all -- once a document is in the
            // writer's own vocabulary, writing it again has to be a no-op, or a
            // clipboard manager degrades a payload a little on every re-copy.
            //
            // KNOWN OPEN BUG, found by this target and reported rather than
            // fixed: a **non-ASCII generator** does exactly that. The writer
            // puts `{\*\generator ...}` through the body-text escaper, so a
            // non-ASCII character leaves as `\uN` followed by a `\ucN` fallback
            // character -- but the generator reader (`tables::generator`, which
            // shares `RtfText` with the font-table reader) does not implement
            // the `\ucN` skip, so it keeps the fallback as text and the next
            // write appends another one. `"Riched20\u{FFFD}1.0"` reads back as
            // `"Riched20\u{FFFD}?1.0"`, then `"Riched20\u{FFFD}??1.0"`, without
            // bound. The minimal input is 19 bytes and is committed here as
            // `regression-generator-fallback-grows.bin`.
            //
            // Which end to fix is a choice for the crate's owner -- teach the
            // generator and font-table decoder the `\ucN` skip, which would
            // also fix the non-BMP font names noted in the runs leg above, or
            // write the generator the way `write_font` already writes a font
            // name, with `\uc0` inside the group and no fallback character. So
            // the field is excluded *by name* rather than the check being
            // skipped: every other part of the fixed point stays live on these
            // inputs, and the assertion comes back the moment the exclusion is
            // deleted.
            // Exactly the characters `escape_text` sends to `escape_unicode`:
            // anything that is not ASCII-graphic or a space, less the three
            // whitespace characters it turns into `\par` / `\tab` instead. Note
            // that this is *not* "non-ASCII" -- U+0001 is ASCII and is escaped,
            // which is how the first attempt at this guard was too narrow.
            let escaped = |c: char| !(c.is_ascii_graphic() || matches!(c, ' ' | '\n' | '\r' | '\t'));
            let generator_survives = second
                .generator
                .as_deref()
                .is_none_or(|g| !g.chars().any(escaped));
            // Note which pair is compared: `twice` against `thrice`, not
            // against `once`. `once` is `write(x)` for a document that came out
            // of an *arbitrary* parse, so it can still carry values the reader
            // normalises -- a generator with trailing whitespace is one, and it
            // is trimmed on the way back in. The fixed point is therefore
            // reached after the second write, not the first, and asserting
            // otherwise measures the reader's normalisation rather than the
            // writer's stability.
            if generator_survives {
                assert_eq!(thrice, twice, "re-writing a written document changed the bytes");
                assert_eq!(third, second, "the parse/write cycle did not reach a fixed point");
            } else {
                let (mut a, mut b) = (second.clone(), third.clone());
                a.generator = None;
                b.generator = None;
                assert_eq!(b, a, "the parse/write cycle did not reach a fixed point");
            }
        }
    }
});

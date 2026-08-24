//! `rclip_html::Runs` — the element stack, the style inheritance and the break
//! rules on top of the lexer.
//!
//! This is where the crate's contract lives, and the contract is unusually
//! strong: **`DepthLimit` is the only error there is.** Mismatched nesting, a
//! stray end tag, an unterminated attribute, a `<` that begins nothing and
//! invalid UTF-8 are all absorbed by design, because they are the *normal* case
//! in clipboard HTML rather than an edge case. So any other `ErrorKind` coming
//! out of here is a bug by definition, and this target asserts exactly that
//! rather than the usual "did not panic".
//!
//! The other half is the stack itself. `Runs` repairs nesting by searching a
//! fixed-size array and reconstructing formatting elements, and both walks are
//! documented as single passes that cannot loop. `<b><i></b></i>` repeated,
//! `<p>` without end tags and `</></>...` are the shapes that exercise them;
//! they are seeded rather than left to chance.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_html::{ErrorKind, RunText, Runs};

fuzz_target!(|data: &[u8]| {
    let mut runs = 0usize;
    let mut errored = false;
    let mut last_offset = 0usize;
    let mut text_bytes = 0usize;

    for run in Runs::new(data) {
        // Documented as terminal. A `Runs` that kept yielding after a
        // `DepthLimit` would mean a caller draining the iterator gets runs from
        // a stack it has already lost track of.
        assert!(!errored, "Runs yielded after a terminal error");
        runs += 1;

        let run = match run {
            Ok(r) => r,
            Err(e) => {
                // The whole contract, in one assertion.
                assert_eq!(
                    e.kind,
                    ErrorKind::DepthLimit,
                    "rclip-html produced an error other than DepthLimit: {e:?}"
                );
                assert!(e.offset <= data.len(), "error offset past the buffer");
                errored = true;
                continue;
            }
        };

        assert!(run.offset <= data.len(), "run offset past the buffer");
        // Runs come out in input order. A run that moved backwards would mean
        // the queued-text mechanism handed back a stale offset, which is what a
        // caller highlighting the source would index with.
        assert!(run.offset >= last_offset, "run offsets went backwards");
        last_offset = run.offset;

        match run.text {
            RunText::Text(t) => {
                let decoded: String = t.chars().collect();
                if let Some(s) = t.as_str() {
                    assert_eq!(s, decoded, "as_str disagreed with chars");
                }
                // `Runs` filters out spans that decode to nothing, so a text
                // run that decodes to nothing is a run the caller has to
                // special-case for no reason.
                assert!(!decoded.is_empty(), "empty text run was emitted");
                text_bytes += t.as_raw().len();
                assert!(text_bytes <= data.len(), "text runs outran the input");
            }
            RunText::Break | RunText::Tab => {}
        }

        let s = run.style;
        // The style's font family borrows from the input; decoding it must not
        // depend on anything but the bytes it points at.
        if let Some(f) = s.font_family {
            let _: String = f.chars().collect();
        }
        let _ = (s.bold, s.italic, s.underline, s.strike, s.size_pt);
        let _ = (s.color, s.background, s.is_default());

        // A break-only document still cannot produce more runs than the input
        // has bytes: every run traces back to a token, and every token consumed
        // at least one byte. `+ 2` covers the break and tab a single token can
        // queue in front of its text.
        assert!(
            runs <= 2 * data.len() + 2,
            "more runs than the input can justify"
        );
    }

    // Draining twice must give the same answer: `Runs` holds no state outside
    // itself, so a second pass that differs would mean it read something it
    // does not own.
    let first: Vec<_> = Runs::new(data).map(|r| r.map(|x| (x.offset, x.style))).collect();
    let second: Vec<_> = Runs::new(data).map(|r| r.map(|x| (x.offset, x.style))).collect();
    assert!(first == second, "two passes over the same bytes disagreed");
});

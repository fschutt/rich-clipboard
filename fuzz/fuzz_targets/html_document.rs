//! `rclip_html::Document::parse` — the owning, merged-run representation.
//!
//! A different entry point from `Runs`, not a wrapper: it materialises the
//! decoded text into one `String`, converts every borrowed `Style` into an
//! `OwnedStyle`, merges adjacent runs whose formatting is identical, and then
//! walks back over the result trimming trailing spaces and popping runs that
//! the trim emptied. That last pass mutates ranges after they were built, which
//! is exactly where a covering invariant goes wrong.
//!
//! The invariant is the one a caller indexes with: the runs cover `text` in
//! order, with no gaps, no overlaps, and every boundary on a UTF-8 character
//! boundary. A violation is a slicing panic in the consumer rather than here,
//! which is why it is asserted rather than left to `run_text` to absorb.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_html::{Document, ErrorKind, RunText, Runs};

fuzz_target!(|data: &[u8]| {
    let doc = match Document::parse(data) {
        Ok(d) => {
            // `Document` and `Runs` are two entry points over one machine, so
            // they must agree about whether these bytes are parseable at all.
            assert!(
                Runs::new(data).all(|r| r.is_ok()),
                "Document::parse succeeded where Runs failed"
            );
            d
        }
        Err(e) => {
            assert_eq!(
                e.kind,
                ErrorKind::DepthLimit,
                "rclip-html produced an error other than DepthLimit: {e:?}"
            );
            assert!(
                Runs::new(data).any(|r| r.is_err()),
                "Document::parse failed where Runs succeeded"
            );
            return;
        }
    };

    let mut end = 0usize;
    for run in &doc.runs {
        assert_eq!(run.range.start, end, "run ranges left a gap or overlapped");
        assert!(run.range.start < run.range.end, "empty run was kept");
        assert!(run.range.end <= doc.text.len(), "run range past the text");
        assert!(
            doc.text.is_char_boundary(run.range.start) && doc.text.is_char_boundary(run.range.end),
            "run range split a UTF-8 character"
        );
        // Panics if the range is not a char boundary, hence the check above.
        assert_eq!(doc.run_text(run).len(), run.range.len());
        end = run.range.end;

        // Adjacent runs are documented as merged, so two neighbours with the
        // same formatting mean the merge missed one and the caller gets a
        // document with one run per tag.
        let _ = run.style.is_default();
        if let Some(f) = &run.style.font_family {
            // An empty family is filtered out on the way in; keeping one would
            // mean a run that claims a font and names none.
            assert!(!f.is_empty(), "empty font family survived");
        }
    }
    assert_eq!(end, doc.text.len(), "runs did not cover the whole text");

    for pair in doc.runs.windows(2) {
        assert_ne!(pair[0].style, pair[1].style, "adjacent runs were not merged");
    }

    // The trailing-space trim is the pass that rewrites ranges after the fact.
    assert!(!doc.text.ends_with(' '), "trailing space survived the trim");

    // The text is the decoded characters of the run stream, in order, and
    // nothing else. Building it here independently is the check that the
    // merge and the trim did not drop or duplicate a span.
    let mut expected = String::new();
    for run in Runs::new(data).flatten() {
        match run.text {
            RunText::Text(t) => match t.as_str() {
                Some(s) => expected.push_str(s),
                None => expected.extend(t.chars()),
            },
            RunText::Break => expected.push('\n'),
            RunText::Tab => expected.push('\t'),
        }
    }
    while expected.ends_with(' ') {
        expected.pop();
    }
    assert_eq!(doc.text, expected, "Document::text is not the run stream");

    let _ = doc.is_plain();
});

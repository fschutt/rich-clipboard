//! `rclip_rtf::Document::parse` — the owning, merged-run representation.
//!
//! A different entry point from `Parser`, not a wrapper: it runs the colour,
//! font and generator scanners as three extra passes and then merges adjacent
//! runs, all of which allocate from lengths the input controls.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_rtf::Document;

fuzz_target!(|data: &[u8]| {
    let Ok(doc) = Document::parse(data) else {
        return;
    };

    // The runs are documented as covering `text` in order, with no gaps and no
    // overlaps. That invariant is what a caller indexes `text` with, so a
    // violation is a slicing panic waiting to happen in the caller.
    let mut end = 0usize;
    for run in &doc.runs {
        assert_eq!(run.range.start, end, "run ranges left a gap or overlapped");
        assert!(run.range.end <= doc.text.len(), "run range past the text");
        assert!(
            doc.text.is_char_boundary(run.range.start) && doc.text.is_char_boundary(run.range.end),
            "run range split a UTF-8 character"
        );
        // Panics if the range is not a char boundary, hence the check above.
        assert_eq!(doc.run_text(run).len(), run.range.len());
        end = run.range.end;
    }
    if !doc.runs.is_empty() {
        assert_eq!(end, doc.text.len(), "runs did not cover the whole text");
    }

    for f in &doc.fonts {
        let _ = doc.font(f.id);
    }
    for (i, _) in doc.colors.iter().enumerate() {
        let _ = doc.color(i as u16);
    }
    let _ = (&doc.generator, doc.codepage, doc.default_font);
});

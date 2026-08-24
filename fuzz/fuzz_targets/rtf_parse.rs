//! RTF body text — `rclip_rtf::Parser`, the borrowing pull parser.
//!
//! Coverage note: `Parser::new` requires a `{\rtf` / `{\urtf` signature, so a
//! naive mutator that has not yet learned those five bytes never gets past the
//! first branch. The corpus seeds supply them, and `rtf_tokenize` drives the
//! same machinery through `Parser::unchecked` with no magic gate at all — see
//! the note there.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_rtf::{header, is_rtf, tables, Codepage, Parser, RunText};

fuzz_target!(|data: &[u8]| {
    let sig = is_rtf(data);
    let head = header(data);

    // The cheap sniffer and the header reader must agree about what is RTF at
    // all: a `header()` that succeeds where `is_rtf()` says no would mean a
    // caller that gates on the sniffer silently drops a valid paste.
    let mut codepage = Codepage::default();
    if let Ok(h) = head {
        assert!(sig, "header() succeeded on bytes is_rtf() rejected");
        codepage = h.codepage;
        let _ = (h.version, h.unicode_variant, h.default_font);
    }

    let mut parser = match Parser::new(data) {
        Ok(p) => p,
        Err(_) => {
            assert!(!sig, "Parser::new rejected bytes is_rtf() accepted");
            return;
        }
    };

    // Drain the run stream. An `Err` is documented as terminal, so a parser
    // that keeps yielding after one is a liveness bug.
    let mut errored = false;
    for run in &mut parser {
        assert!(!errored, "parser yielded a run after a terminal error");
        match run {
            Ok(r) => {
                let _ = (r.offset, r.props.is_plain(), r.props.points());
                if let RunText::Text(s) = r.text {
                    // Documented as always ASCII: RTF is a 7-bit format and a
                    // byte >= 0x80 is a code-page byte that must come back as
                    // `Char`, not be smuggled into a borrowed `&str`.
                    assert!(s.is_ascii(), "literal run was not ASCII: {s:?}");
                }
                let _ = r.text.is_empty();
            }
            Err(_) => errored = true,
        }
    }
    let _ = (parser.codepage(), parser.default_font());

    // The table scanners are separate passes over the same bytes, each with its
    // own brace tracking, and each reachable independently of the body parse.
    for c in tables::colors(data) {
        let _ = c;
    }
    for f in tables::fonts(data, codepage) {
        let _ = (f.id, f.family, f.charset);
        let _ = f.name.as_str();
        for ch in f.name.chars() {
            let _ = ch;
        }
    }
    if let Some(g) = tables::generator(data, codepage) {
        let _ = g.as_str();
        for ch in g.chars() {
            let _ = ch;
        }
    }
});

//! `CF_HTML` ("HTML Format") — `rclip_cf_html::parse` / `parse_detailed`.
//!
//! Every offset in a `CF_HTML` header is an attacker-controlled index into the
//! very buffer the header sits in, so this is the target where a slicing bug
//! would show up first.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_cf_html::{parse, parse_detailed, CfHtmlBuilder};

fuzz_target!(|data: &[u8]| {
    // No panic, no hang. `parse` returns `Result`, so a panic is a bug by
    // construction.
    let simple = parse(data);
    let detailed = parse_detailed(data);

    // The two entry points must agree: `parse` is documented as
    // `parse_detailed(..).map(|p| p.content)`.
    assert_eq!(simple.is_ok(), detailed.is_ok());

    let Ok(p) = detailed else { return };
    assert_eq!(simple.unwrap(), p.content);

    // Everything the header claimed must be a real, in-bounds view of `data`.
    for s in [Some(p.content.fragment), p.content.context, p.content.selection]
        .into_iter()
        .flatten()
    {
        assert!(is_subslice(data, s.as_bytes()), "view escaped the input buffer");
    }
    if let Some((a, b)) = p.selection_in_fragment {
        // The field is documented as "the selection's byte range within
        // `CfHtml::fragment`", so do exactly what it exists for. Slicing rather
        // than only bounds-checking is the point: it is the call a consumer
        // makes, and `&fragment[a..b]` panics on a bad *char boundary* as well
        // as on a bad length.
        //
        // This found a real bug on its first long run: `resolve` raises a
        // `StartFragment` that points inside the header up to the end of the
        // header, but `selection_in_fragment` was subtracting the *unclamped*
        // start, so a 124-byte blob reported `(0, 124)` for a 9-byte fragment.
        // Fixed in crates/rclip-cf-html/src/parse.rs; see
        // `regression-selection-past-fragment.bin` in this target's corpus.
        assert!(a <= b, "selection range runs backwards");
        let sel = &p.content.fragment[a..b];
        if let Some(text) = p.content.selection {
            assert_eq!(sel, text, "selection_in_fragment does not name the selection");
        }
    }

    // Round trip. Lossy by design: the format carries two redundant encodings
    // of the fragment boundary (marker comments and byte offsets) and the
    // builder always emits both in agreement, so a payload where they
    // disagreed cannot come back byte-identical. The property asserted is
    // therefore semantic — the fragment, the source URL and the selection
    // survive — not `serialize(parse(x)) == x`.
    let mut b = CfHtmlBuilder::new(p.content.fragment).version(p.content.version);
    match p.content.context {
        // The context is a whole document with the fragment somewhere inside
        // it; the builder wants the two halves around the fragment. Splitting
        // it back out is only unambiguous when the fragment occurs once, so
        // feed the default wrapper otherwise and only assert the fragment.
        Some(_) => {}
        None => b = b.no_context(),
    }
    if let Some(url) = p.content.source_url {
        b = b.source_url(url);
    }
    if let Some((s, e)) = p.selection_in_fragment {
        b = b.selection(s..e);
    }

    let Ok(blob) = b.build() else {
        // The builder legitimately refuses input the parser accepts: a
        // fragment that already contains a marker comment, a source URL with
        // an embedded newline. Refusing is the correct answer, not a bug.
        return;
    };
    let back = parse(&blob).expect("a blob we just built must parse");
    assert_eq!(back.fragment, p.content.fragment);
    assert_eq!(back.source_url, p.content.source_url);
    assert_eq!(back.version, p.content.version);
    if p.selection_in_fragment.is_some() {
        assert_eq!(back.selection, p.content.selection);
    }

    // And it must be a fixed point: building from what we just parsed twice
    // gives the same bytes.
    let again = CfHtmlBuilder::new(back.fragment)
        .version(back.version);
    let again = match back.source_url {
        Some(u) => again.source_url(u),
        None => again,
    };
    let again = match p.selection_in_fragment {
        Some((s, e)) => again.selection(s..e),
        None => again,
    };
    let again = match p.content.context {
        Some(_) => again,
        None => again.no_context(),
    };
    assert_eq!(again.build().expect("rebuild"), blob);
});

/// `true` if `part` is a view into `whole` (or is empty).
fn is_subslice(whole: &[u8], part: &[u8]) -> bool {
    if part.is_empty() {
        return true;
    }
    let base = whole.as_ptr() as usize;
    let p = part.as_ptr() as usize;
    p >= base && p + part.len() <= base + whole.len()
}

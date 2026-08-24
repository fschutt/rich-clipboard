//! The `style=` attribute reader — `rclip_html::css`.
//!
//! Its own target rather than a branch of `html_tokenize`, because reaching it
//! through the tokenizer costs the mutator a well-formed `<div style="…">`
//! wrapper before a single declaration byte matters. Fed directly, every
//! mutation lands inside the block.
//!
//! The splitter is the interesting half. It tracks quotes *and* parentheses
//! *and* decodes character references as it goes — `font-family:&quot;Foo
//! Bar&quot;` is one declaration, and a splitter that treated the `;` of
//! `&quot;` as a separator would cut it into three. That entity call inside the
//! scanner is also the one place where a bad `len` would move the splitter's
//! cursor by an amount the input chose, so the progress assertion below is
//! doing real work.
//!
//! The value readers under it are pure functions over a byte slice, and each
//! one is reachable on any input, so they are all driven on every declaration.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_html::{css, ColorValue};

/// Parent sizes for the relative units. `em`, `rem`, `ex`, `ch` and `%` all
/// resolve against the enclosing element, so a `None` parent and a hostile one
/// are different arithmetic — including the two that are not numbers.
const PARENTS: [Option<f32>; 6] = [
    None,
    Some(css::DEFAULT_PT),
    Some(0.0),
    Some(-1.0),
    Some(f32::MAX),
    Some(f32::NAN),
];

fuzz_target!(|data: &[u8]| {
    let mut count = 0usize;
    let mut consumed = 0usize;
    for decl in css::declarations(data) {
        count += 1;
        // Every iteration takes at least one byte plus its separator, so a
        // declaration count above the block's length means the splitter stood
        // still -- an infinite loop dressed up as a timeout.
        assert!(count <= data.len() + 1, "more declarations than bytes");

        // Both halves are slices of the block, so neither can be longer than
        // it, and together they cannot outrun it.
        assert!(decl.name.len() <= data.len());
        assert!(decl.value.len() <= data.len());
        consumed += decl.name.len() + decl.value.len();
        assert!(consumed <= data.len(), "declarations outran the block");
        // Deliberately *not* asserted: that `decl.name` is non-empty.
        // `Declarations::next` skips a declaration whose trimmed name is empty
        // and then converts that name with `from_utf8(..).unwrap_or("")`, so a
        // name that is bytes rather than text arrives as a declaration with an
        // empty name -- past the guard that exists to prevent exactly that.
        // Harmless in effect (an empty name matches no property, so the
        // declaration is ignored either way) but it is those two lines
        // disagreeing, and the raw name bytes are not reachable from
        // `Declaration`, so there is nothing here to assert against.
        // `regression-non-utf8-name.bin` keeps the shape in the corpus.

        let _ = decl.is("color");
        let _ = decl.is("FONT-FAMILY");

        let _ = css::font_weight_bold(decl.value);
        let _ = css::font_style_italic(decl.value);
        if let Some(d) = css::text_decoration(decl.value) {
            let _ = (d.underline, d.strike);
        }
        match css::color(decl.value) {
            Some(ColorValue::Rgb(c)) => {
                let _ = (c.r, c.g, c.b);
            }
            // `transparent` is a third outcome and not a missing one: it clears
            // an inherited background rather than painting black.
            Some(ColorValue::Transparent) => {}
            None => {}
        }
        for parent in PARENTS {
            if let Some(pt) = css::font_size_pt(decl.value, parent) {
                // A size the caller will put in a `\fsN` or a layout box. NaN
                // and infinity both propagate silently through arithmetic and
                // surface much later as a blank line or a hung layout, so they
                // are refused here rather than downstream.
                assert!(pt.is_finite(), "font-size resolved to {pt}");
                assert!(pt > 0.0, "font-size resolved to a non-positive {pt}");
            }
        }
        if let Some(pt) = css::font_attr_size_pt(decl.value) {
            assert!(pt.is_finite() && pt > 0.0, "<font size> resolved to {pt}");
        }
        let _ = css::named_color(decl.value);

        // `trim` and `unquote` are idempotent by construction; a second pass
        // that changed the answer would mean one of them consumed a byte it
        // should not have.
        let t = css::trim(decl.value);
        assert_eq!(css::trim(t), t, "trim was not idempotent");
        assert!(t.len() <= decl.value.len());
        let u = css::unquote(t);
        assert!(u.len() <= t.len());
    }

    // The same block read as a bare value, which is what the presentational
    // attributes (`<font size>`, `bgcolor`) hand these functions.
    if let Some(pt) = css::font_attr_size_pt(data) {
        assert!(pt.is_finite() && pt > 0.0, "<font size> resolved to {pt}");
    }
    let _ = css::named_color(css::trim(data));
    let _ = css::color(data);
    for parent in PARENTS {
        if let Some(pt) = css::font_size_pt(data, parent) {
            assert!(pt.is_finite() && pt > 0.0, "font-size resolved to {pt}");
        }
    }
});

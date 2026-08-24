//! Character references — `rclip_html::entity::decode`.
//!
//! Called at *every* offset rather than only at the `&`s, so the out-of-range
//! and mid-sequence paths are reached without the mutator having to place an
//! ampersand first. This is the smallest entry point in the crate and the one
//! with the most arithmetic in it, which is a good trade for a fuzz target.
//!
//! Two rules make it worth its own target rather than being left to
//! `html_tokenize`:
//!
//! * **Longest match with a missing semicolon.** `&notin` is `&notin;` and not
//!   `&not;` followed by `in`, which means the scanner tries every prefix
//!   length down from the longest. Getting the loop bound wrong there consumes
//!   bytes that were not part of the reference — text the paste then loses.
//! * **The Windows-1252 remap.** HTML5 mandates that `&#150;` is an en dash
//!   rather than U+0096, because that is what every browser had to implement.
//!   Five values in `0x80..=0x9F` are deliberately *not* remapped and decode to
//!   themselves. Both halves are asserted below, because a table edit that
//!   dropped an entry would otherwise be invisible.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_html::entity;

/// The five C1 values HTML5 leaves alone. Everything else in `0x80..=0x9F` is
/// a printable Windows-1252 character and must not come back as a control.
const UNMAPPED_C1: [u32; 5] = [0x81, 0x8D, 0x8F, 0x90, 0x9D];

/// The numeric value a `&#…` reference at `at` names, if it is one, together
/// with whether it is well-formed enough for the value to be meaningful.
///
/// Re-derived here rather than asked of the crate, because the point is to
/// check the crate's mapping against the spec's and a shared helper would check
/// nothing.
fn numeric_value(input: &[u8], at: usize) -> Option<u32> {
    let body = input.get(at + 1..)?;
    if body.first() != Some(&b'#') {
        return None;
    }
    let (digits, radix) = match body.get(1) {
        Some(b'x' | b'X') => (body.get(2..)?, 16u32),
        _ => (body.get(1..)?, 10u32),
    };
    let mut value: u32 = 0;
    let mut used = 0usize;
    for &b in digits {
        let d = match (radix, b) {
            (_, b'0'..=b'9') => u32::from(b - b'0'),
            (16, b'a'..=b'f') => u32::from(b - b'a') + 10,
            (16, b'A'..=b'F') => u32::from(b - b'A') + 10,
            _ => break,
        };
        value = value.saturating_mul(radix).saturating_add(d);
        used += 1;
        // Past this the crate stops accumulating on purpose; so does this.
        if used > 8 {
            return None;
        }
    }
    if used == 0 {
        return None;
    }
    Some(value)
}

fuzz_target!(|data: &[u8]| {
    // One past the end and a long way past it: `decode` takes an offset from
    // the caller and must reject an out-of-range one rather than index with it.
    for at in [data.len(), data.len() + 1, usize::MAX] {
        assert!(
            entity::decode(data, at).is_none(),
            "decoded a reference past the end of the buffer"
        );
    }

    for at in 0..data.len() {
        let Some(reference) = entity::decode(data, at) else {
            // Only a `&` can start a reference. Everything else must decline.
            continue;
        };
        assert_eq!(data[at], b'&', "decoded a reference that did not start at &");

        // The two bounds that make the result usable as a cursor advance. A
        // `len` of zero would make `HtmlChars` spin; a `len` past the end would
        // make it skip past the buffer.
        assert!(reference.len >= 2, "reference consumed less than `&x`");
        assert!(
            at + reference.len <= data.len(),
            "reference claimed bytes past the end: len {} at {at} of {}",
            reference.len,
            data.len()
        );
        assert_ne!(reference.ch, '\0', "NUL is not a character reference");

        // A reference is `&`, an optional `#` and `x`, alphanumerics, and an
        // optional trailing `;`. Anything else in the consumed span means the
        // scanner ran past the syntax and swallowed document text.
        let span = &data[at..at + reference.len];
        for (i, &b) in span.iter().enumerate() {
            let ok = match b {
                b'&' => i == 0,
                b';' => i == reference.len - 1,
                b'#' => i == 1,
                // `x` is the hex marker at index 2 and an ordinary letter
                // anywhere else -- `&boxh;` has one at index 3.
                _ => b.is_ascii_alphanumeric(),
            };
            assert!(ok, "reference span carried {b:#x} at {i}: {span:?}");
        }

        match numeric_value(data, at) {
            Some(value) => {
                // The Windows-1252 remap, both directions.
                if (0x80..=0x9F).contains(&value) {
                    if UNMAPPED_C1.contains(&value) {
                        assert_eq!(
                            u32::from(reference.ch),
                            value,
                            "a C1 value HTML5 leaves alone was remapped anyway"
                        );
                    } else {
                        assert!(
                            !(0x80..=0x9F).contains(&u32::from(reference.ch)),
                            "&#{value}; decoded to a C1 control instead of Windows-1252"
                        );
                    }
                }
                // Surrogates, NUL and anything past the last scalar value all
                // become U+FFFD; `char` cannot hold them, so the only thing to
                // check is that the crate did not produce something else.
                if value == 0 || value > 0x10_FFFF || (0xD800..=0xDFFF).contains(&value) {
                    assert_eq!(
                        reference.ch,
                        char::REPLACEMENT_CHARACTER,
                        "an unrepresentable value did not become U+FFFD"
                    );
                }
            }
            None => {
                // A named reference. `MAX_NAME` is 32, plus the `&` and the
                // optional `;`: a longer span means the scan is not bounded and
                // `&` followed by four kilobytes of letters is quadratic.
                let numeric_shaped = data.get(at + 1) == Some(&b'#');
                if !numeric_shaped {
                    assert!(
                        reference.len <= 34,
                        "named reference ran past the 32-byte name bound: {}",
                        reference.len
                    );
                    // Longest match: with no `;`, a shorter prefix may also
                    // resolve, but a *longer* one must not — that is what
                    // "longest defined prefix wins" means, and getting it
                    // backwards is how `&notin` becomes `&not;` + `in`.
                    let has_semi = span.last() == Some(&b';');
                    if !has_semi {
                        let name_len = data
                            .get(at + 1..)
                            .unwrap_or_default()
                            .iter()
                            .take(32)
                            .take_while(|b| b.is_ascii_alphanumeric())
                            .count();
                        assert!(
                            reference.len - 1 <= name_len,
                            "reference consumed more than the name run it matched"
                        );
                    }
                }
            }
        }
    }
});

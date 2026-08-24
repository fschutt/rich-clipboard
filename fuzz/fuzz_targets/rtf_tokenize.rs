//! The RTF tokenizer and the unchecked parser entry point.
//!
//! Why this exists next to `rtf_parse`: `Parser::new` gates on a `{\rtf`
//! signature at offset 0, which makes naive mutation nearly useless against
//! everything behind it — the mutator has to reinvent five exact bytes before a
//! single new edge is reachable. `Tokenizer::new` and `Parser::unchecked` have
//! no such gate, so this target reaches the control-word scanner, the `\'hh`
//! and `\uN` escape decoders, the `\bin` payload skipper and the group stack on
//! *any* input. It broadens the target rather than narrowing it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_rtf::{ControlSymbol, Parser, Token, Tokenizer};

fuzz_target!(|data: &[u8]| {
    let mut depth: i64 = 0;
    let mut errored = false;
    for tok in Tokenizer::new(data) {
        // `Tokenizer` documents that "an `Err` is terminal: the iterator
        // fuses afterwards rather than resynchronising". This target found the
        // one path where it did not -- `control_word`'s four `?` returns, of
        // which `\binN` past the end is the reachable one -- and
        // `regression-bin-past-end-does-not-fuse.bin` in this target's corpus
        // is the 6-byte input that showed it. Fixed by routing that `Result`
        // through `fail()`; this assertion is what keeps it fixed.
        assert!(!errored, "tokenizer yielded a token after a terminal error");
        match tok {
            Ok(Token::GroupStart) => depth += 1,
            Ok(Token::GroupEnd) => depth -= 1,
            Ok(Token::ControlWord { name, param }) => {
                // Deliberately *not* asserting the spec's 32-letter cap: the
                // crate documents that it does not enforce it, because a longer
                // word simply fails every lookup. What must hold is that the
                // scanner only ever hands back the letter run it matched.
                assert!(
                    name.bytes().all(|b| b.is_ascii_alphabetic()),
                    "control word had a non-letter: {name:?}"
                );
                let _ = param;
            }
            Ok(Token::Text(s)) => {
                // Documented as ASCII and free of the three structural
                // characters. A `{` or `}` inside a text token means the lexer
                // lost track of the group nesting.
                assert!(s.is_ascii(), "text token was not ASCII: {s:?}");
                assert!(
                    !s.contains(['\\', '{', '}']),
                    "text token carried a structural character: {s:?}"
                );
            }
            Ok(Token::ControlSymbol(s)) => {
                let _ = matches!(s, ControlSymbol::NonBreakingSpace);
            }
            Ok(other) => {
                let _ = other;
            }
            Err(_) => errored = true,
        }
        // The tokenizer does not enforce balance -- the parser does -- but a
        // depth that runs away without the input growing would be a loop.
        assert!(depth.unsigned_abs() as usize <= data.len() + 1);
    }

    // Same bytes through the parser with the signature check skipped, which is
    // the documented entry point for a fragment.
    //
    // `Parser` *does* fuse correctly -- it funnels every tokenizer error
    // through its own `fail()` -- so this assertion stays live, and it is the
    // one that matters for a caller: `Parser` is the entry point a paste
    // handler uses.
    let mut p = Parser::unchecked(data);
    let mut errored = false;
    for run in &mut p {
        assert!(!errored, "parser yielded a run after a terminal error");
        match run {
            Ok(r) => {
                let _ = (r.offset, r.props);
                let _ = r.text.is_empty();
            }
            Err(_) => errored = true,
        }
    }
});

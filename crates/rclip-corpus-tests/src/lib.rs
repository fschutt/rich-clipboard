//! Whole-corpus checks for `rich-clipboard`.
//!
//! Twelve codec crates each sweep their own fixture directory. That checks the
//! corpus twelve times and never once as a whole, which leaves two things
//! invisible:
//!
//! 1. **Orphans.** A `.bin` with no sidecar, a sidecar whose `.bin` was
//!    deleted, a whole directory nobody routes — all of it passes today,
//!    because a per-crate sweep only ever looks in its own directory.
//! 2. **Leaks.** A captured fixture is cut from a real machine. One in this
//!    corpus already had to be scrubbed by hand after it turned out to carry a
//!    boot-volume UUID and a sandbox HMAC. That was caught by a human reading a
//!    report.
//!
//! This crate is the gate for both. It is `publish = false` and its library
//! half has no dependencies at all: a corpus walker, a JSON reader for the one
//! shape a sidecar has, and a byte scanner. The parsers live in the tests,
//! which are the only place the codec crates are needed.
//!
//! - [`walk`] — the corpus as one tree.
//! - [`sidecar`] — the contract from `corpus/README.md`, as code.
//! - [`json`] — the sidecar reader.
//! - [`scan`] — the leak scanner.
//!
//! See `tests/corpus_gate.rs` and `tests/leak_scan.rs`.

#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

pub mod json;
pub mod scan;
pub mod sidecar;
pub mod walk;

pub use walk::{corpus_root, walk, Corpus, Fixture, Orphan};

/// Collect a list of failures into one panic message.
///
/// Every check in this crate reports everything it found rather than stopping
/// at the first problem. Fixing a corpus one failed assertion per `cargo test`
/// run is how a sweep becomes a chore, and a chore becomes a `#[ignore]`.
pub fn report(headline: &str, problems: &[String]) {
    if problems.is_empty() {
        return;
    }
    let mut msg = format!("\n{headline} ({} problems)\n\n", problems.len());
    for p in problems {
        msg.push_str("  - ");
        msg.push_str(&p.replace('\n', "\n    "));
        msg.push('\n');
    }
    panic!("{msg}");
}

//! RTF 1.9.1 reader for clipboard-grade styled text.
//!
//! Scoped to what a clipboard carries — runs of text with character formatting
//! — and not to what a word processor needs. On macOS `public.rtf` is *the*
//! rich flavor (Pages, TextEdit, Mail and Notes all speak it and several speak
//! no HTML at all); on Windows Word and Outlook offer it alongside `CF_HTML`
//! and it is the higher-fidelity of the two. See `plan/PLAN.md` §4.3.
//!
//! # Layers
//!
//! ```text
//! bytes ──▶ Tokenizer ──▶ Parser ──▶ StyledRun        (no_std, borrowing)
//!                     └─▶ fonts() / colors() / generator()
//!                                     └──▶ Document   (feature "alloc")
//! ```
//!
//! [`Tokenizer`] is a pure lexer with no state beyond a cursor. [`Parser`] adds
//! the group stack, the destination rules and the `\ucN` skip counter, and
//! yields [`StyledRun`]s that borrow from the input.
//!
//! # The `alloc` boundary
//!
//! Parsing never allocates. The one thing that cannot be done without an
//! allocation is handing back *decoded* text as a single string: `\uN` and
//! `\'hh` escapes mean the characters of a document are not a contiguous slice
//! of its bytes anywhere. So:
//!
//! - **Without `alloc`**: [`StyledRun`] carries either a borrowed `&str` (the
//!   common case — most text is literal) or a single decoded `char`. Runs are
//!   not merged, because merging needs somewhere to put the joined text.
//! - **With `alloc`**: [`Document`] owns one `String` of plain text plus
//!   [`Run`]s that are byte ranges into it, with adjacent equal-property runs
//!   merged.
//!
//! # Example
//!
//! ```
//! use rclip_rtf::{Parser, RunText};
//!
//! // `\uc1` says one ASCII fallback character follows every `\uN`. The `-`
//! // after `\u8212` is that fallback: a reader that does not honour the skip
//! // count pastes an extra hyphen into the user's document.
//! let src = br"{\rtf1\ansi\uc1 plain \b bold\b0 \u8212-done}";
//!
//! let mut all = String::new();
//! let mut bolded = String::new();
//! for run in Parser::new(src).unwrap() {
//!     let run = run.unwrap();
//!     let piece = match run.text {
//!         RunText::Text(s) => s.to_string(),
//!         RunText::Char(c) => c.to_string(),
//!         _ => "\n".to_string(),
//!     };
//!     if run.props.bold {
//!         bolded.push_str(&piece);
//!     }
//!     all.push_str(&piece);
//! }
//!
//! assert_eq!(all, "plain bold\u{2014}done");
//! assert_eq!(bolded, "bold");
//! ```
//!
//! # Writing
//!
//! [`Writer`] is the inverse, behind `alloc`. It takes *resolved* formatting —
//! a font name and an RGB colour rather than a `\fN` / `\cfN` index — interns
//! the two tables itself, and emits a minimal document. Every non-ASCII
//! character leaves as `\uN` with a one-character ASCII fallback and never as a
//! raw byte, because the reader on the other end may be running under a
//! different `\ansicpg` than this document declares. [`Document::to_rtf`] is
//! the round-tripping form: it writes a parsed document's own tables back
//! verbatim, so `Document::parse(&doc.to_rtf())` returns an equal `Document`.
//!
//! # Not implemented in phase 0
//!
//! - **Paragraph properties.** `\pard` is accepted and resets nothing, because
//!   alignment, indents and spacing are not modelled. `// TODO(phase-1):`
//! - **Tables, lists, fields and pictures.** `\trowd`, `\pn`, `\field` and
//!   `\pict` contribute no structure; the visible text of a field
//!   (`\fldrslt`) does come through.
//! - **Code pages other than Windows-1252 and Latin-1** are behind the optional
//!   `codepage` feature. With it on, `\mac`, `\pc`, `\pca` and every
//!   `\ansicpgN` naming a Windows-125x page decode through `rclip-codepage`;
//!   with it off they still yield U+FFFD rather than a guess. The CJK pages
//!   Word emits for `\fcharset` fonts are multi-byte and out of scope either
//!   way. See [`Codepage`].
//! - **`\upr`.** The ANSI half is read and the `{\*\ud}` Unicode half skipped,
//!   which is the behaviour the construct was designed to give old readers.
//!   `// TODO(phase-1):` prefer the `\ud` half.
//! - **Underline styles.** Every `\ul*` collapses to a boolean.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod codepage;
pub mod control;
pub mod parse;
pub mod style;
pub mod tables;
pub mod token;

pub use codepage::Codepage;
pub use parse::{header, is_rtf, Header, Parser, RunText, StyledRun};
/// Re-exported so a caller can name a code page without adding `rclip-codepage`
/// to its own manifest.
#[cfg(feature = "codepage")]
pub use rclip_codepage::Encoding;
pub use style::{CharProps, Color, Font, FontFamily, RtfChars, RtfText};
pub use tables::{colors, fonts, generator, ColorTable, FontTable};
pub use token::{ControlSymbol, Token, Tokenizer};

pub use rclip_core::{Error, ErrorKind, Result};

#[cfg(feature = "alloc")]
pub mod document;
#[cfg(feature = "alloc")]
pub mod write;

#[cfg(feature = "alloc")]
pub use document::{Document, OwnedFont, Run};
#[cfg(feature = "alloc")]
pub use write::{half_points, write, FontDef, WriteProps, Writer};

//! HTML tokenizer for clipboard-grade styled text.
//!
//! Turns the `text/html` / `public.html` / `CF_HTML` fragment a source put on
//! the clipboard into runs of text with character formatting. `no_std`,
//! `forbid(unsafe_code)`, borrowing, and no dependency but `rclip-core`.
//!
//! # This is not a browser
//!
//! The scope is one sentence: **enough HTML to recover the styling a clipboard
//! fragment carries, and nothing else.** Concretely, what is here is a
//! tokenizer, an element stack that repairs mismatched nesting, character
//! references, and enough of a `style=` attribute reader to see the seven
//! properties [`Style`] has room for. What is *not* here is a DOM, a cascade, a
//! selector engine, `<style>` rules, layout, scripting, foreign content
//! (`<svg>`, `<math>`), the HTML5 insertion modes, or the adoption agency
//! algorithm. See the crate README for the line-by-line boundary.
//!
//! # Layers
//!
//! ```text
//! bytes ──▶ Tokenizer ──▶ Runs ──▶ Run + Style        (no_std, borrowing)
//!                    └─▶ css::declarations
//!                                └──▶ Document        (feature "alloc")
//! ```
//!
//! [`Tokenizer`] is a lexer with a cursor and one flag. [`Runs`] adds the
//! element stack, the inheritance and the break rules.
//!
//! # The `alloc` boundary
//!
//! Parsing never allocates. What cannot be done without an allocation is
//! handing back the *decoded* text as one string, because `&amp;` is one
//! character written as five bytes and a run of indentation is one space, so
//! the characters of a fragment are not a contiguous slice of its bytes
//! anywhere. So [`HtmlText`] is a lazy view with an `as_str` fast path, and
//! [`Document`] behind `alloc` owns one `String` plus runs that are byte ranges
//! into it, with adjacent equal-style runs merged.
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "alloc")] {
//! use rclip_html::Document;
//!
//! // Mismatched nesting, an entity, a style attribute and a block break —
//! // which is to say, a normal clipboard fragment.
//! let doc = Document::parse(
//!     br#"<p>Fish <b><i>&amp; chips</b></i></p><p style="color:#ff0000">next</p>"#,
//! )
//! .unwrap();
//!
//! assert_eq!(doc.text, "Fish & chips\nnext");
//! assert!(doc.runs[1].style.bold && doc.runs[1].style.italic);
//! assert_eq!(doc.runs.last().unwrap().style.color, Some(rclip_html::Color::new(255, 0, 0)));
//! # }
//! ```
//!
//! # Errors
//!
//! There is exactly one: [`ErrorKind::DepthLimit`], when element nesting
//! exceeds [`rclip_core::MAX_DEPTH`]. Everything else is absorbed. That is not
//! leniency for its own sake — a clipboard payload is written by another
//! process and parsed the moment the user presses Ctrl+V, and malformed
//! nesting, unterminated attributes and stray `<` are the *normal* case in
//! clipboard HTML rather than an edge case.
//!
//! # Not implemented
//!
//! - **`<style>` rules and the cascade.** A fragment that styles its text
//!   through a class arrives unstyled. Browsers inline the styles onto the
//!   elements when they write a clipboard fragment precisely so that the
//!   receiving application does not need a cascade.
//! - **Hyperlinks, images, lists, tables as structure, superscript and
//!   subscript.** [`Style`] has no room for any of them; see its docs.
//! - **Character encodings other than UTF-8.** A `<meta charset>` is not read.
//!   The clipboard flavors this serves are defined as UTF-8, and the one that
//!   is not — `text/html`, where producers write UTF-16 with and without a BOM
//!   — is sniffed by the caller before the bytes reach here.
//! - **CSS `white-space`.** `<pre>` and `<textarea>` preserve whitespace;
//!   everything else collapses it. `white-space: pre` in a `style=` attribute
//!   is not read.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

pub mod css;
pub mod element;
pub mod entity;
pub mod parse;
pub mod style;
pub mod text;
pub mod token;

pub use css::{ColorValue, Declaration, Declarations};
pub use element::Formatting;
pub use parse::{Run, RunText, Runs};
pub use style::{Color, Style};
pub use text::{HtmlChars, HtmlText, Whitespace};
pub use token::{Attr, Attrs, Tag, Token, Tokenizer};

pub use rclip_core::{Error, ErrorKind, Result};

#[cfg(feature = "alloc")]
pub mod document;
#[cfg(feature = "alloc")]
pub use document::{Document, OwnedStyle};

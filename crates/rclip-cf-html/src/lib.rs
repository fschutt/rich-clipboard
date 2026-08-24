//! `CF_HTML` — the Windows "HTML Format" registered clipboard format.
//!
//! Spec: [HTML Clipboard Format][spec]. A `CF_HTML` payload is an **ASCII
//! description header** followed by a **UTF-8 body**:
//!
//! ```text
//! Version:1.0
//! StartHTML:0000000121
//! EndHTML:0000000272
//! StartFragment:0000000147
//! EndFragment:0000000247
//! StartSelection:0000000180      (optional, both or neither)
//! EndSelection:0000000225
//! SourceURL:https://example.com/ (optional, not in the current grammar)
//! <html><!--StartFragment-->…<!--EndFragment--></html>
//! ```
//!
//! # What this crate is careful about
//!
//! Every one of these is a trap that has bitten a shipping implementation:
//!
//! - **Offsets are absolute.** They count bytes from the start of the *whole
//!   blob*, header included — not from the end of the header. `StartHTML` is
//!   therefore normally equal to the header length.
//! - **Offsets may be zero-padded to any width.** The spec explicitly blesses
//!   `StartHTML:0000000121`, because that is how a producer reserves room to
//!   back-patch the number it does not know yet. A value of `0000000000` is
//!   the number zero, not a parse error — stripping zeros and then parsing the
//!   empty string is a real bug in a real crate.
//! - **The numbers and the comments disagree in the wild.** Microsoft's own
//!   documented MSHTML example has `StartFragment:0006` / `EndFragment:0106`
//!   where the `<!--StartFragment-->` comments sit at 147 and 247. This crate
//!   trusts the comments and reports the disagreement through
//!   [`Parsed::fragment_source`], so a caller can see it happened.
//! - **`StartHTML`/`EndHTML` may be `-1`,** meaning "fragment only, no
//!   context". See [`Offset::Negative`].
//! - **Line endings may be `\r\n`, `\n`, or a lone `\r`.** `str::lines` does
//!   not split on a lone `\r`, so it cannot be used here.
//! - **Unknown header keys must be skipped, not rejected.** The spec reserves
//!   the right to extend the header, and Internet Explorer already did, with
//!   `SourceURL`.
//!
//! # Reading
//!
//! ```
//! # fn main() -> Result<(), rclip_cf_html::Error> {
//! let blob = b"Version:1.0\r\nStartHTML:0000000105\r\nEndHTML:0000000177\r\n\
//! StartFragment:0000000137\r\nEndFragment:0000000145\r\n\
//! <html><body><!--StartFragment-->hi there<!--EndFragment--></body></html>";
//! let html = rclip_cf_html::parse(blob)?;
//! assert_eq!(html.fragment, "hi there");
//! assert!(html.context.unwrap().starts_with("<html><body>"));
//! # Ok(())
//! # }
//! ```
//!
//! # Writing
//!
//! The offsets in a `CF_HTML` header refer to positions in the very buffer the
//! header is part of, so writing one is self-referential. [`CfHtmlBuilder`]
//! resolves that the way the spec suggests: emit a fixed-width ten-digit
//! placeholder for every offset, then overwrite the digits in place once the
//! buffer is complete. Because the field width never changes, nothing shifts
//! and no second pass is needed. Iterating toward a fixed point — writing the
//! offsets, noticing the header got longer, and writing them again — is the
//! mistake this design exists to avoid.
//!
//! [spec]: https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod header;
mod marker;
mod parse;
#[cfg(feature = "alloc")]
mod serialize;

pub use header::{Header, Offset, Version};
pub use parse::{parse, parse_detailed, CfHtml, FragmentSource, Parsed};
#[cfg(feature = "alloc")]
pub use serialize::CfHtmlBuilder;

pub use rclip_core::{Error, ErrorKind, Result};

// TODO(phase-2): converting the fragment into the workspace's shared
// `RichText` type. That needs an HTML tokenizer, which belongs in its own
// crate; this one's job ends at handing back a `&str` of markup. Nothing here
// parses, validates or normalizes HTML, and the spec's "a valid fragment is a
// single outer element" rule is deliberately not enforced — clipboard
// producers break it constantly and a paste that refuses their markup is worse
// than one that passes it along.

/// The string to hand `RegisterClipboardFormat` to get the `CF_HTML` format id.
///
/// There is no fixed `CF_*` number for `CF_HTML`; it is a *registered* format,
/// so its numeric id differs per session and must be looked up by this name.
pub const FORMAT_NAME: &str = rclip_core::flavor::cfstr::HTML;

/// The verbatim fragment-start marker comment, as this crate writes it.
///
/// The spec requires "no whitespace chars within each comment itself" — and
/// then contradicts itself two sections later with a grammar that spells it
/// `<!--StartFragment -->`, and again in its own scenarios with
/// `<!-- StartFragment-->`. The parser therefore accepts whitespace anywhere
/// inside the comment; the serializer emits only this strict form.
pub const START_FRAGMENT_COMMENT: &str = "<!--StartFragment-->";

/// The verbatim fragment-end marker comment, as this crate writes it.
///
/// See [`START_FRAGMENT_COMMENT`] for why the parser is laxer than this.
pub const END_FRAGMENT_COMMENT: &str = "<!--EndFragment-->";

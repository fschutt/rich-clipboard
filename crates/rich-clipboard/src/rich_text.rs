//! The lossy-conversion hub.
//!
//! `plan/PLAN.md` §4.3 and its open decision 3 both call for one intermediate
//! representation rather than pairwise conversions, and the arithmetic is the
//! whole argument: RTF, `CF_HTML`, `text/html` and azul's `ClipboardContent`
//! are four representations of styled text, so pairwise is twelve conversions
//! and a hub is eight — and the next format added costs two rather than eight.
//!
//! # What [`RichText`] can represent
//!
//! One flat run of text with **character-level** formatting: bold, italic,
//! underline, strikethrough, point size, font family, foreground and background
//! colour. Paragraph breaks are `\n` in [`RichText::text`]. That is deliberately
//! the intersection of what a clipboard actually carries between unrelated
//! applications, not the union of what any one format can express.
//!
//! # What it cannot
//!
//! Everything structural. Paragraph alignment, indents and spacing; lists;
//! tables; inline images; hyperlinks; superscript and subscript; underline
//! *style* (dotted, wavy, double — all of them collapse to a boolean, because
//! `rclip-rtf` collapses them and HTML's `text-decoration` does not agree with
//! RTF's list anyway); anything a format models as a field or an object.
//!
//! This is a real ceiling and not a phase-1 shortcut. A hub that could represent
//! everything would be a document model, and every conversion into it from a
//! format that has less would have to invent the difference. Losing structure
//! at a documented boundary beats fabricating it at an undocumented one.
//!
//! # Both legs
//!
//! `RichText` converts both ways with RTF (through `rclip-rtf`) and both ways
//! with HTML (through `rclip-html`), so decoding an HTML flavor produces styled
//! runs rather than markup. It did not until phase 2: `rclip-cf-html` reads the
//! Windows header and states outright that it does not parse markup, and until
//! `rclip-html` existed there was nothing in the workspace that did.
//!
//! What HTML still loses on the way in is the cascade — a fragment that styles
//! its text through a class rather than through a `style=` attribute arrives
//! unstyled — plus links, images, and lists and tables as structure. That is
//! why [`Flavor::read_rank`](rclip_core::Flavor::read_rank) still puts RTF
//! above HTML: when a source offers both, which Word, Outlook and LibreOffice
//! all do, the read side takes the one that needs no stylesheet.
//!
//! A caller that wants the markup itself rather than the styling can still have
//! it — [`Options::keep_html_markup`](crate::Options::keep_html_markup) turns
//! the decode back into a [`RichItem::Html`](crate::RichItem::Html), which is
//! what a clipboard bridge or an inspector wants and what carries `SourceURL`
//! and the surrounding context document.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

/// An 8-bit sRGB colour.
///
/// No alpha: neither RTF's `\colortbl` nor the CSS this crate writes carries
/// one for text, and a field that is always opaque is a field that will
/// eventually be believed.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// Construct a colour.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Character formatting.
///
/// [`Default`] means "whatever the receiving application uses for body text" —
/// not black 12pt. Every field is an explicit override, which is why the
/// optional ones are `Option` rather than a sentinel: a run that does not name
/// a colour should inherit one, and a run that names black should get black
/// even on a dark background.
//
// Deliberately *not* `#[non_exhaustive]`. This is the type a consumer builds by
// hand more than any other, and `Style { bold: true, ..Style::default() }` is
// the line they will write; `#[non_exhaustive]` forbids exactly that from
// outside the crate. Adding a field here is a semver break, and that is the
// honest trade for a struct whose whole job is to be written down.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Style {
    /// `\b` / `<b>` / `font-weight: 700`.
    pub bold: bool,
    /// `\i` / `<i>` / `font-style: italic`.
    pub italic: bool,
    /// `\ul` / `<u>`. Every underline *style* collapses here; see the module
    /// docs.
    pub underline: bool,
    /// `\strike` / `<s>`.
    pub strikethrough: bool,
    /// Size in points. `None` inherits.
    ///
    /// RTF stores half-points (`\fs24` is 12pt) and always has a value, so a
    /// `RichText` that came from RTF always has this set — including when the
    /// producer simply never overrode the default.
    pub size_pt: Option<f32>,
    /// Font family name, as the producer spelled it. Not resolved against any
    /// font on this machine, because there is no machine here.
    pub font_family: Option<String>,
    /// Text colour. `None` inherits.
    pub color: Option<Rgb>,
    /// Background / highlight colour. `None` inherits.
    pub background: Option<Rgb>,
}

impl Style {
    /// `true` if this run overrides nothing.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A styled range of [`RichText::text`].
///
/// A byte range rather than an owned `String`, so the plain text stays one
/// contiguous buffer. That buffer is exactly what `CF_UNICODETEXT` and
/// `public.utf8-plain-text` want, and the plain-text flavor is published
/// alongside the rich one every single time — duplicating the characters into
/// per-run strings would mean rebuilding it on every encode.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledRun {
    /// Byte range into [`RichText::text`]. Always on character boundaries.
    pub range: Range<usize>,
    /// The formatting in effect over that range.
    pub style: Style,
}

/// Styled text: one string, and the formatting spans over it.
///
/// The runs are in order, non-overlapping, and cover the whole of `text` with
/// no gaps — [`RichText::push`] is the only way to grow one, and it maintains
/// that. A `RichText` with empty `runs` and non-empty `text` is not
/// constructible through the public API.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RichText {
    /// The characters, with paragraph and line breaks as `\n`.
    pub text: String,
    /// The formatting spans, in order.
    pub runs: Vec<StyledRun>,
}

impl RichText {
    /// Unstyled text.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        let mut out = Self::default();
        out.push(&text.into(), Style::default());
        out
    }

    /// Append `text` under `style`, merging into the previous run when the
    /// style is identical.
    ///
    /// Merging is not cosmetic. `rclip-rtf` cuts a run at every group boundary
    /// and every escape, so an RTF document that is one sentence in one font
    /// can arrive as one run per character; a writer that emitted a `\b0\b` pair
    /// between each of them would produce output several times the size of the
    /// input for no visible difference.
    pub fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        match self.runs.last_mut() {
            Some(last) if last.style == style => last.range.end = end,
            _ => self.runs.push(StyledRun {
                range: start..end,
                style,
            }),
        }
    }

    /// The plain text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// `true` if nothing is styled — the whole thing would survive a trip
    /// through `CF_UNICODETEXT` unchanged.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        self.runs.iter().all(|r| r.style.is_default())
    }

    /// `true` if there is no text at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The characters of one run.
    ///
    /// Empty rather than panicking if `run` came from a different `RichText`.
    #[must_use]
    pub fn run_text(&self, run: &StyledRun) -> &str {
        self.text.get(run.range.clone()).unwrap_or_default()
    }

    /// Iterate `(text, style)` pairs.
    pub fn spans(&self) -> impl Iterator<Item = (&str, &Style)> {
        self.runs
            .iter()
            .map(move |r| (self.text.get(r.range.clone()).unwrap_or_default(), &r.style))
    }
}

impl From<&str> for RichText {
    fn from(s: &str) -> Self {
        Self::plain(s)
    }
}

impl From<String> for RichText {
    fn from(s: String) -> Self {
        Self::plain(s)
    }
}

// ---------------------------------------------------------------------------
// RTF
// ---------------------------------------------------------------------------

#[cfg(feature = "rtf")]
mod rtf_conv {
    use alloc::string::String;
    use alloc::vec::Vec;

    use rclip_rtf::Document;

    use super::{Rgb, RichText, Style};

    impl RichText {
        /// Decode RTF.
        ///
        /// # What is lost
        ///
        /// Everything under "what it cannot represent" in the module docs, plus
        /// what `rclip-rtf` itself does not read: paragraph properties, tables,
        /// lists, fields, pictures, `\upr`'s Unicode half, and any code page
        /// other than Windows-1252 and Latin-1 (those decode to U+FFFD rather
        /// than being guessed at). Cell and row boundaries survive as a tab and
        /// a newline so text does not run together.
        ///
        /// # Errors
        ///
        /// [`rclip_core::Error`] for a bad signature, unbalanced braces, or
        /// nesting past [`rclip_core::MAX_DEPTH`]. Everything recoverable — an
        /// unknown control word, an undecodable byte — is absorbed, because a
        /// paste that drops one character beats a paste that fails.
        pub fn from_rtf(bytes: &[u8]) -> rclip_core::Result<Self> {
            let doc = Document::parse(bytes)?;
            let mut out = Self::default();
            for run in &doc.runs {
                let props = &run.props;
                let font = props
                    .font
                    .or(doc.default_font)
                    .and_then(|id| doc.font(id))
                    .map(|f| String::from(f.name.as_str()))
                    .filter(|n| !n.is_empty());
                let style = Style {
                    bold: props.bold,
                    italic: props.italic,
                    underline: props.underline,
                    strikethrough: props.strike,
                    size_pt: Some(props.points()),
                    font_family: font,
                    color: props.foreground.and_then(|i| doc.color(i)).map(to_rgb),
                    background: props.background.and_then(|i| doc.color(i)).map(to_rgb),
                };
                out.push(doc.run_text(run), style);
            }
            Ok(out)
        }

        /// Encode as RTF 1.9.1.
        ///
        /// # What is lost
        ///
        /// Nothing that [`RichText`] holds. The output is a minimal document:
        /// a `\fonttbl` and a `\colortbl` containing only what the runs
        /// actually reference, no paragraph properties, no generator
        /// destination.
        ///
        /// Non-ASCII is always written as `\uN` with an ASCII fallback and
        /// never as a raw high byte — the reader on the other end may be under
        /// a different `\ansicpg` than we assumed, and a raw byte would arrive
        /// as a different character rather than as a visible gap.
        #[must_use]
        pub fn to_rtf(&self) -> Vec<u8> {
            let mut w = rclip_rtf::Writer::new();
            for (text, style) in self.spans() {
                w.push(
                    text,
                    &rclip_rtf::WriteProps {
                        bold: style.bold,
                        italic: style.italic,
                        underline: style.underline,
                        strike: style.strikethrough,
                        size_half_points: style.size_pt.map(rclip_rtf::half_points),
                        font: style.font_family.as_deref(),
                        foreground: style.color.map(from_rgb),
                        background: style.background.map(from_rgb),
                    },
                );
            }
            w.finish()
        }
    }

    fn to_rgb(c: rclip_rtf::Color) -> Rgb {
        Rgb::new(c.red, c.green, c.blue)
    }

    fn from_rgb(c: Rgb) -> rclip_rtf::Color {
        rclip_rtf::Color::new(c.r, c.g, c.b)
    }
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

#[cfg(feature = "html")]
mod html_conv {
    use alloc::string::String;
    use core::fmt::Write as _;

    use super::{Rgb, RichText, Style};

    impl RichText {
        /// Decode an HTML fragment into styled runs.
        ///
        /// # What is lost
        ///
        /// Everything under "what it cannot represent" in the module docs, plus
        /// what `rclip-html` itself does not do: `<style>` rules and the
        /// cascade, so text styled through a class rather than through a
        /// `style=` attribute arrives unstyled; hyperlinks; images; lists and
        /// tables as structure. Block boundaries become `\n` and table cell
        /// boundaries `\t`, so nothing runs together.
        ///
        /// Browsers inline the styles onto the elements when they put a
        /// fragment on the clipboard, which is exactly why the missing cascade
        /// is a floor and not a hole.
        ///
        /// # Errors
        ///
        /// [`rclip_core::Error`] with [`ErrorKind::DepthLimit`] and nothing
        /// else. Mismatched nesting, unterminated attributes, stray `<` and
        /// invalid UTF-8 are all absorbed: in clipboard HTML they are the
        /// normal case rather than the exception.
        ///
        /// [`ErrorKind::DepthLimit`]: rclip_core::ErrorKind::DepthLimit
        pub fn from_html(markup: &str) -> rclip_core::Result<Self> {
            Self::from_html_bytes(markup.as_bytes())
        }

        /// Decode an HTML fragment that is already bytes.
        ///
        /// The bytes must be UTF-8. A `text/html` payload off a real clipboard
        /// might not be — see [`crate::decode`], which sniffs the encoding
        /// first and then comes here.
        ///
        /// # Errors
        ///
        /// As [`RichText::from_html`].
        pub fn from_html_bytes(markup: &[u8]) -> rclip_core::Result<Self> {
            let doc = rclip_html::Document::parse(markup)?;
            let mut out = Self::default();
            for run in &doc.runs {
                let style = Style {
                    bold: run.style.bold,
                    italic: run.style.italic,
                    underline: run.style.underline,
                    strikethrough: run.style.strike,
                    size_pt: run.style.size_pt,
                    font_family: run.style.font_family.clone(),
                    color: run.style.color.map(to_rgb),
                    background: run.style.background.map(to_rgb),
                };
                out.push(doc.run_text(run), style);
            }
            Ok(out)
        }

        /// Render as an HTML fragment.
        ///
        /// One `<span>` per styled run, with the formatting as inline CSS, and
        /// bare escaped text for a run that overrides nothing. Inline styles
        /// rather than `<b>`/`<i>` because a fragment lands inside a document
        /// whose stylesheet we have never seen, and `<b>` there means whatever
        /// that stylesheet says it means.
        ///
        /// # What is lost
        ///
        /// Nothing that [`RichText`] holds. Note that the result is a
        /// *fragment*, not a document: no `<html>`, no `<meta charset>`. Wrap
        /// it with [`RichText::to_cf_html`] for Windows, or hand it to
        /// `text/html` as-is everywhere else, where the flavor is defined to be
        /// UTF-8 markup.
        #[must_use]
        pub fn to_html_fragment(&self) -> String {
            let mut out = String::new();
            for (text, style) in self.spans() {
                if style.is_default() {
                    escape_into(&mut out, text);
                    continue;
                }
                out.push_str("<span style=\"");
                css_into(&mut out, style);
                out.push_str("\">");
                escape_into(&mut out, text);
                out.push_str("</span>");
            }
            out
        }

        /// Render as a `CF_HTML` blob for the Windows clipboard.
        ///
        /// `source_url` becomes the `SourceURL` header, which is the only way a
        /// consumer can resolve a relative link in the fragment. Pass it when
        /// you have it.
        ///
        /// # Errors
        ///
        /// [`rclip_core::Error`] if the source URL contains a line break, which
        /// would inject a header line, or if the blob would exceed the
        /// ten-digit offset fields.
        pub fn to_cf_html(
            &self,
            source_url: Option<&str>,
        ) -> rclip_core::Result<alloc::vec::Vec<u8>> {
            let fragment = self.to_html_fragment();
            let mut builder = rclip_cf_html::CfHtmlBuilder::new(&fragment);
            if let Some(url) = source_url {
                builder = builder.source_url(url);
            }
            builder.build()
        }
    }

    fn to_rgb(c: rclip_html::Color) -> Rgb {
        Rgb::new(c.r, c.g, c.b)
    }

    fn escape_into(out: &mut String, s: &str) {
        for ch in s.chars() {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                // Escaped even though a text node does not need it, because the
                // same routine writes attribute values.
                '"' => out.push_str("&quot;"),
                '\n' => out.push_str("<br>"),
                c => out.push(c),
            }
        }
    }

    fn css_into(out: &mut String, style: &Style) {
        if style.bold {
            out.push_str("font-weight:700;");
        }
        if style.italic {
            out.push_str("font-style:italic;");
        }
        // One declaration, because a second `text-decoration` overrides the
        // first rather than adding to it.
        match (style.underline, style.strikethrough) {
            (true, true) => out.push_str("text-decoration:underline line-through;"),
            (true, false) => out.push_str("text-decoration:underline;"),
            (false, true) => out.push_str("text-decoration:line-through;"),
            (false, false) => {}
        }
        if let Some(pt) = style.size_pt {
            // `{:.1}` rather than `{}` so a half-point size does not print as
            // `10.5000001pt`, and so 12.0 prints as `12.0` rather than `12`.
            let _ = write!(out, "font-size:{pt:.1}pt;");
        }
        if let Some(name) = style.font_family.as_deref() {
            out.push_str("font-family:&quot;");
            escape_into(out, name);
            out.push_str("&quot;;");
        }
        if let Some(c) = style.color {
            let _ = write!(out, "color:#{:02x}{:02x}{:02x};", c.r, c.g, c.b);
        }
        if let Some(c) = style.background {
            let _ = write!(out, "background-color:#{:02x}{:02x}{:02x};", c.r, c.g, c.b);
        }
    }
}

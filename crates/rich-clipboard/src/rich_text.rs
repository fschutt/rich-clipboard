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
//! # The missing leg
//!
//! `RichText` converts **to** HTML and **both ways** with RTF. It does not
//! convert *from* HTML: that needs an HTML tokenizer, `rclip-cf-html` states
//! outright that it does not parse markup, and nothing else in the workspace
//! does either. Decoding an HTML flavor therefore yields
//! [`RichItem::Html`](crate::RichItem::Html) — the markup, intact — rather than
//! a `RichText`. `// TODO(phase-2):` an `rclip-html` tokenizer closes this.
//!
//! It is less of a hole than it looks, because it is exactly why
//! [`Flavor::read_rank`](rclip_core::Flavor::read_rank) puts RTF above HTML:
//! when a source offers both — which Word, Outlook and LibreOffice all do — the
//! read side takes the one that becomes structure.

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
            super::rtf_write::write(self)
        }
    }

    fn to_rgb(c: rclip_rtf::Color) -> Rgb {
        Rgb::new(c.red, c.green, c.blue)
    }
}

// The RTF *writer* lives here rather than in `rclip-rtf` because that crate
// does not have one yet: its `lib.rs` carries `// TODO(phase-2): the writer`.
// The facade needs one — on macOS `public.rtf` is the only rich flavor most
// applications read, so a fan-out that cannot produce RTF cannot publish styled
// text there at all.
//
// TODO(phase-2): delete this module and call `rclip_rtf`'s writer once it
// exists. Everything below is deliberately written to the same rules that
// crate's README states for the writer it is going to grow, so the swap is a
// deletion rather than a behaviour change.
#[cfg(feature = "rtf")]
mod rtf_write {
    use alloc::vec::Vec;

    use super::{Rgb, RichText, Style};

    /// Half-points. `\fs24` is 12pt, and is the RTF default.
    ///
    /// The `+ 0.5` is a rounding step done by hand: `f32::round` lives in
    /// `std`, and this crate keeps the `no_std + alloc` door open because azul
    /// targets wasm. A cast truncates toward zero, so for a positive value the
    /// two are the same thing.
    fn half_points(pt: f32) -> u16 {
        let hp = pt * 2.0 + 0.5;
        if hp.is_finite() && hp >= 1.0 && hp < f32::from(u16::MAX) {
            hp as u16
        } else {
            // Not a size we can write. The RTF default, which is 12pt.
            24
        }
    }

    pub(super) fn write(text: &RichText) -> Vec<u8> {
        // Collect the tables first: RTF wants them in the header, before any
        // body text, and a run refers to them by index.
        let mut fonts: Vec<&str> = Vec::new();
        // Index 0 of `\colortbl` is conventionally the omitted "auto" entry,
        // which is what `\cf0` means. Real colours start at 1.
        let mut colors: Vec<Rgb> = Vec::new();
        for run in &text.runs {
            if let Some(name) = run.style.font_family.as_deref() {
                if !fonts.contains(&name) {
                    fonts.push(name);
                }
            }
            for c in [run.style.color, run.style.background]
                .into_iter()
                .flatten()
            {
                if !colors.contains(&c) {
                    colors.push(c);
                }
            }
        }

        let mut out = Vec::new();
        out.extend_from_slice(br"{\rtf1\ansi\ansicpg1252\uc1\deff0");

        out.extend_from_slice(br"{\fonttbl");
        // `\f0` always exists so `\deff0` resolves — a reader that cannot find
        // `\deffN` in the table is entitled to do anything at all — and its
        // name is deliberately *empty*: it stands for "whatever the reader uses
        // for body text", which is what `Style::font_family: None` means.
        // Naming a real font here would invent a statement the caller never
        // made, and it would come back as one on the next parse.
        out.extend_from_slice(br"{\f0\fnil ;}");
        for (i, name) in fonts.iter().enumerate() {
            out.extend_from_slice(br"{\f");
            push_num(&mut out, i + 1);
            out.extend_from_slice(br"\fnil ");
            escape_into(&mut out, name);
            out.extend_from_slice(b";}");
        }
        out.push(b'}');

        if !colors.is_empty() {
            // The leading `;` is the auto entry. Dropping it shifts every index
            // by one and recolours the whole document.
            out.extend_from_slice(br"{\colortbl;");
            for c in &colors {
                out.extend_from_slice(br"\red");
                push_num(&mut out, usize::from(c.r));
                out.extend_from_slice(br"\green");
                push_num(&mut out, usize::from(c.g));
                out.extend_from_slice(br"\blue");
                push_num(&mut out, usize::from(c.b));
                out.push(b';');
            }
            out.push(b'}');
        }

        out.extend_from_slice(b"\n");

        for run in &text.runs {
            // `\plain` resets every character property, so each run states its
            // own formatting in full and nothing leaks across a boundary.
            out.extend_from_slice(br"\plain");
            emit_props(&mut out, &run.style, &fonts, &colors);
            out.push(b' ');
            escape_into(&mut out, text.run_text(run));
        }

        out.push(b'}');
        out
    }

    fn emit_props(out: &mut Vec<u8>, style: &Style, fonts: &[&str], colors: &[Rgb]) {
        if let Some(name) = style.font_family.as_deref() {
            if let Some(i) = fonts.iter().position(|f| *f == name) {
                out.extend_from_slice(br"\f");
                push_num(out, i + 1);
            }
        }
        out.extend_from_slice(br"\fs");
        push_num(out, usize::from(half_points(style.size_pt.unwrap_or(12.0))));
        if style.bold {
            out.extend_from_slice(br"\b");
        }
        if style.italic {
            out.extend_from_slice(br"\i");
        }
        if style.underline {
            out.extend_from_slice(br"\ul");
        }
        if style.strikethrough {
            out.extend_from_slice(br"\strike");
        }
        if let Some(i) = style
            .color
            .and_then(|c| colors.iter().position(|k| *k == c))
        {
            out.extend_from_slice(br"\cf");
            push_num(out, i + 1);
        }
        if let Some(i) = style
            .background
            .and_then(|c| colors.iter().position(|k| *k == c))
        {
            // `\highlight` rather than `\cb`: Word writes `\cb` and reads
            // `\highlight`, and `rclip-rtf` accepts either.
            out.extend_from_slice(br"\highlight");
            push_num(out, i + 1);
        }
    }

    fn push_num(out: &mut Vec<u8>, n: usize) {
        let mut buf = [0u8; 20];
        let mut i = buf.len();
        let mut n = n;
        loop {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        out.extend_from_slice(&buf[i..]);
    }

    /// Escape text into the body of an RTF document.
    ///
    /// The three metacharacters are escaped, `\n` and `\t` become control
    /// words, and everything outside ASCII becomes `\uN?`. The trailing `?` is
    /// the one-character ANSI fallback that `\uc1` in the header promises; a
    /// reader that honours the skip count drops it, and one that does not shows
    /// a question mark instead of mojibake.
    fn escape_into(out: &mut Vec<u8>, s: &str) {
        for ch in s.chars() {
            match ch {
                '\\' => out.extend_from_slice(br"\\"),
                '{' => out.extend_from_slice(br"\{"),
                '}' => out.extend_from_slice(br"\}"),
                // A paragraph break. `\line` would be a soft break inside one;
                // `RichText` does not distinguish, and `\par` is what a
                // receiver turns back into a newline.
                '\n' => out.extend_from_slice(br"\par "),
                '\t' => out.extend_from_slice(br"\tab "),
                // Dropped rather than escaped: `rclip-rtf` treats a bare CR as
                // a stream artefact, so writing one would round-trip to
                // nothing anyway.
                '\r' => {}
                c if c.is_ascii_graphic() || c == ' ' => out.push(c as u8),
                c => {
                    let mut buf = [0u16; 2];
                    for unit in c.encode_utf16(&mut buf) {
                        // `\uN`'s parameter is a signed 16-bit value, so
                        // anything above 32767 — every surrogate, among other
                        // things — is written as its negative wraparound.
                        out.extend_from_slice(br"\u");
                        let signed = i32::from(*unit as i16);
                        if signed < 0 {
                            out.push(b'-');
                            push_num(out, signed.unsigned_abs() as usize);
                        } else {
                            push_num(out, signed as usize);
                        }
                        out.push(b'?');
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

#[cfg(feature = "html")]
mod html_conv {
    use alloc::string::String;
    use core::fmt::Write as _;

    use super::{RichText, Style};

    impl RichText {
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

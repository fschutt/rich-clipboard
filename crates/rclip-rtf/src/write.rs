//! The writer: styled runs to RTF 1.9.1 bytes. Feature `alloc`.
//!
//! The inverse of [`crate::Parser`], and the traps are the inverse too. Four
//! rules do the work, and each of them is a way real RTF writers corrupt text:
//!
//! 1. **Nothing outside ASCII is ever written as a byte.** Every non-ASCII
//!    character becomes `\uN` with a one-character ASCII fallback. A `\'hh`
//!    escape or a raw high byte would only decode correctly under the code page
//!    this document happens to declare, and the reader on the other end may be
//!    running under a different one — in which case a raw byte arrives as a
//!    *different character* rather than as a visible gap.
//! 2. **`\uc1` is stated in the header and honoured everywhere.** The counter
//!    says how many characters follow each `\uN` as its fallback, so a writer
//!    that declares `\uc1` and then emits a two-character fallback makes every
//!    reader that respects the counter eat one character of real text. Body
//!    escapes therefore emit exactly one fallback character; the one place a
//!    fallback would be wrong at all — a `\fonttbl` name, which is scanned
//!    verbatim — declares `\uc0` and emits none.
//! 3. **`\fsN` is half-points.** 12pt is `\fs24`. Getting this wrong halves or
//!    doubles every font size in the document, which is the single most visible
//!    way an RTF writer can be wrong.
//! 4. **The first `\colortbl` entry is empty.** `{\colortbl;\red255...}` — the
//!    leading `;` is the "auto" entry that `\cf0` names, and dropping it shifts
//!    every colour index by one and recolours the whole document.
//!
//! # Two entry points
//!
//! [`Writer`] takes *resolved* formatting — a font name, an RGB colour — and
//! builds the tables itself. That is what a caller converting from some other
//! styled-text representation wants, and it is what the `rich-clipboard` facade
//! uses.
//!
//! [`crate::Document::to_rtf`] takes a parsed document and writes its own
//! tables back verbatim, ids and gaps included, so that
//! `Document::parse(&doc.to_rtf())` gives back an equal `Document` rather than
//! an equivalent one with renumbered indices.
//!
//! # What the output does not contain
//!
//! No paragraph properties, no `\deflang`, no style sheet, no `\pict`, no
//! generator unless one is asked for. `RichText`-grade styled text and the two
//! tables it references, and nothing else — see the crate README.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use crate::style::{CharProps, Color, FontFamily};

/// A `\fonttbl` entry, as the writer will emit it.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct FontDef<'a> {
    /// The `N` of `\fN`. A key that runs refer to, not a position in the table.
    pub id: u16,
    /// The family hint. [`FontFamily::Nil`] (`\fnil`) is the honest answer when
    /// all that is known is a name.
    pub family: FontFamily,
    /// `\fcharsetN`. `None` omits it.
    pub charset: Option<u16>,
    /// The name, plain Unicode. Escaped on the way out; leading and trailing
    /// ASCII whitespace does not survive, because the reader trims it.
    pub name: &'a str,
}

/// Character formatting with fonts and colours *resolved*, for [`Writer`].
///
/// The difference from [`CharProps`] is the whole reason this type exists:
/// `CharProps` carries `\fN` / `\cfN` indices, which are only meaningful next
/// to the tables they index. A caller converting from some other representation
/// has a font *name* and an RGB *colour*, and interning those into tables is
/// the writer's job, not the caller's.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct WriteProps<'a> {
    /// `\b`
    pub bold: bool,
    /// `\i`
    pub italic: bool,
    /// `\ul`
    pub underline: bool,
    /// `\strike`
    pub strike: bool,
    /// `\fsN`, in **half-points**: 12pt is 24. `None` writes the RTF default,
    /// which is 24. See [`half_points`].
    pub size_half_points: Option<u16>,
    /// Font family name. `None` means the reader's body font.
    pub font: Option<&'a str>,
    /// `\cfN`. `None` means the reader's default text colour.
    pub foreground: Option<Color>,
    /// `\cbN` / `\highlightN`. `None` means no background.
    pub background: Option<Color>,
}

/// Points to half-points, rounded to nearest. `12.0` is `24`.
///
/// `f32::round` lives in `std` and this crate is `no_std`, so the `+ 0.5` is
/// the rounding step done by hand; a cast truncates toward zero, which for a
/// positive value is the same thing. Anything that is not a size an `\fsN`
/// parameter can hold — negative, infinite, NaN, absurd — becomes the RTF
/// default of 24 rather than a silently clamped number.
#[must_use]
pub fn half_points(points: f32) -> u16 {
    let hp = points * 2.0 + 0.5;
    if hp.is_finite() && hp >= 1.0 && hp < f32::from(u16::MAX) {
        hp as u16
    } else {
        CharProps::DEFAULT.size_half_points
    }
}

/// Builds an RTF document out of styled runs.
///
/// Interns font names and colours into the two tables as runs arrive, merges
/// adjacent runs whose formatting is identical, and writes the whole thing on
/// [`Writer::finish`].
///
/// ```
/// use rclip_rtf::{Color, WriteProps, Writer};
///
/// let mut w = Writer::new();
/// w.push("plain ", &WriteProps::default());
/// w.push(
///     "bold red",
///     &WriteProps {
///         bold: true,
///         foreground: Some(Color::new(255, 0, 0)),
///         ..WriteProps::default()
///     },
/// );
/// let rtf = w.finish();
///
/// let doc = rclip_rtf::Document::parse(&rtf).unwrap();
/// assert_eq!(doc.text, "plain bold red");
/// assert_eq!(doc.runs.len(), 2);
/// assert!(doc.runs[1].props.bold);
/// ```
#[derive(Debug, Clone)]
pub struct Writer<'a> {
    text: String,
    runs: Vec<(Range<usize>, CharProps)>,
    /// Font names by `\fN` id. Entry 0 is deliberately empty; see
    /// [`Writer::finish`].
    fonts: Vec<&'a str>,
    /// Real colours. `\colortbl`'s auto entry is index 0 and is not stored, so
    /// `colors[i]` is written as `\cf(i + 1)`.
    colors: Vec<Color>,
    generator: Option<&'a str>,
}

impl Default for Writer<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Writer<'a> {
    /// An empty document.
    #[must_use]
    pub fn new() -> Self {
        Self {
            text: String::new(),
            runs: Vec::new(),
            // `\f0` with an empty name stands for "whatever the reader uses for
            // body text", which is what `WriteProps::font: None` means. Naming
            // a real font here would invent a statement the caller never made,
            // and it would come back as one on the next parse.
            fonts: alloc::vec![""],
            colors: Vec::new(),
            generator: None,
        }
    }

    /// Set `{\*\generator ...}`.
    ///
    /// Off by default: it is not content, and a minimal document is easier to
    /// diff against a real writer's output. Worth setting when the receiving
    /// end has per-writer workarounds.
    #[must_use]
    pub fn generator(mut self, name: &'a str) -> Self {
        self.generator = Some(name);
        self
    }

    /// `true` if nothing has been pushed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Append `text` under `props`.
    ///
    /// Empty text is ignored. Adjacent pushes with identical formatting are
    /// merged, so a caller replaying `rclip-rtf`'s own runs — which are cut at
    /// every group boundary and every escape — does not produce a `\plain`
    /// preamble per character.
    pub fn push(&mut self, text: &str, props: &WriteProps<'a>) {
        if text.is_empty() {
            return;
        }
        let font = match props.font {
            None => 0,
            Some(name) => match self.fonts.iter().position(|f| *f == name) {
                Some(i) => i as u16,
                None => {
                    // `\fN` is a 16-bit id, so the table stops growing there.
                    // A document with 65 535 distinct font names is not a
                    // clipboard payload; reusing the last id loses a name and
                    // keeps the document valid.
                    if self.fonts.len() >= usize::from(u16::MAX) {
                        (self.fonts.len() - 1) as u16
                    } else {
                        self.fonts.push(name);
                        (self.fonts.len() - 1) as u16
                    }
                }
            },
        };
        let fg = props.foreground.and_then(|c| self.intern_color(c));
        let bg = props.background.and_then(|c| self.intern_color(c));

        let props = CharProps {
            bold: props.bold,
            italic: props.italic,
            underline: props.underline,
            strike: props.strike,
            size_half_points: props
                .size_half_points
                .unwrap_or(CharProps::DEFAULT.size_half_points),
            font: Some(font),
            foreground: fg,
            background: bg,
        };

        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        match self.runs.last_mut() {
            Some((range, last)) if *last == props => range.end = end,
            _ => self.runs.push((start..end, props)),
        }
    }

    /// Table index for `c`, as a `\cfN` parameter (so 1-based).
    fn intern_color(&mut self, c: Color) -> Option<u16> {
        if let Some(i) = self.colors.iter().position(|k| *k == c) {
            return Some((i + 1) as u16);
        }
        if self.colors.len() + 1 >= usize::from(u16::MAX) {
            return None;
        }
        self.colors.push(c);
        Some(self.colors.len() as u16)
    }

    /// Serialize.
    #[must_use]
    pub fn finish(&self) -> Vec<u8> {
        let fonts: Vec<FontDef<'_>> = self
            .fonts
            .iter()
            .enumerate()
            .map(|(i, name)| FontDef {
                id: i as u16,
                family: FontFamily::Nil,
                // `\fcharset0` is ANSI. Stated rather than omitted because a
                // reader that cannot find a charset falls back to its system
                // default, and on a Cyrillic or CJK Windows that is not ANSI.
                charset: Some(0),
                name,
            })
            .collect();
        // The auto entry, then the real colours. Its emptiness is the point:
        // `\cf0` has to keep meaning "the reader's default text colour".
        let mut colors: Vec<Option<Color>> = Vec::with_capacity(self.colors.len() + 1);
        colors.push(None);
        colors.extend(self.colors.iter().copied().map(Some));

        write_document(
            &fonts,
            if self.colors.is_empty() { &[] } else { &colors },
            Some(0),
            self.generator,
            self.runs
                .iter()
                .map(|(r, p)| (self.text.get(r.clone()).unwrap_or_default(), *p)),
        )
    }
}

/// Serialize styled runs as a complete RTF document.
///
/// The one-shot form of [`Writer`], for a caller that already has its runs in
/// hand.
///
/// ```
/// use rclip_rtf::{write, WriteProps};
///
/// let rtf = write([
///     ("hello ", WriteProps::default()),
///     ("world", WriteProps { italic: true, ..WriteProps::default() }),
/// ]);
/// assert_eq!(rclip_rtf::Document::parse(&rtf).unwrap().text, "hello world");
/// ```
#[must_use]
pub fn write<'a, I>(runs: I) -> Vec<u8>
where
    I: IntoIterator<Item = (&'a str, WriteProps<'a>)>,
{
    let mut w = Writer::new();
    for (text, props) in runs {
        w.push(text, &props);
    }
    w.finish()
}

// ---------------------------------------------------------------------------
// The emitter
// ---------------------------------------------------------------------------

/// Write a whole document: header, tables, body.
///
/// `colors` is indexed by `\cfN` directly, so its first entry is the auto entry
/// and is normally `None`. An empty slice writes no `\colortbl` at all, which
/// is only valid if no run names a colour.
pub(crate) fn write_document<'t, I>(
    fonts: &[FontDef<'_>],
    colors: &[Option<Color>],
    default_font: Option<u16>,
    generator: Option<&str>,
    runs: I,
) -> Vec<u8>
where
    I: Iterator<Item = (&'t str, CharProps)>,
{
    let mut out = Vec::new();
    // `\ansicpg1252` is a statement about `\'hh` bytes, of which this writer
    // emits none. It is still written because a reader that sees `\ansi` with
    // no code page falls back to *its own* system code page, and a document
    // that says nothing is a document two readers can disagree about.
    out.extend_from_slice(br"{\rtf1\ansi\ansicpg1252\uc1");
    if let Some(f) = default_font {
        out.extend_from_slice(br"\deff");
        push_u32(&mut out, u32::from(f));
    }

    if !fonts.is_empty() {
        out.extend_from_slice(br"{\fonttbl");
        for font in fonts {
            write_font(&mut out, font);
        }
        out.push(b'}');
    }

    if !colors.is_empty() {
        out.extend_from_slice(br"{\colortbl");
        for entry in colors {
            if let Some(c) = entry {
                out.extend_from_slice(br"\red");
                push_u32(&mut out, u32::from(c.red));
                out.extend_from_slice(br"\green");
                push_u32(&mut out, u32::from(c.green));
                out.extend_from_slice(br"\blue");
                push_u32(&mut out, u32::from(c.blue));
            }
            // Every entry is `;`-terminated, defined or not. That is what makes
            // an omitted entry expressible at all.
            out.push(b';');
        }
        out.push(b'}');
    }

    if let Some(g) = generator {
        out.extend_from_slice(br"{\*\generator ");
        escape_text(&mut out, g);
        out.extend_from_slice(b";}");
    }

    // A newline between the header and the body, which is where every writer
    // puts one and where a reader is guaranteed to ignore it: CR and LF carry
    // no meaning in RTF outside a `\`-escape. No other line breaking is done —
    // a break landing between `\uN` and its fallback character is precisely how
    // a wrapped document loses a character.
    out.push(b'\n');

    for (text, props) in runs {
        if text.is_empty() {
            continue;
        }
        // `\plain` resets every character property, so each run states its
        // formatting in full and nothing leaks across a run boundary. Verified
        // against AppKit: after `\plain` the font, size, colour and every
        // toggle are back to the document default.
        out.extend_from_slice(br"\plain");
        write_props(&mut out, &props);
        // The delimiter. Exactly one space is consumed by a reader, so text
        // that itself starts with a space keeps it.
        out.push(b' ');
        escape_text(&mut out, text);
    }

    out.push(b'}');
    out
}

fn write_font(out: &mut Vec<u8>, font: &FontDef<'_>) {
    out.extend_from_slice(br"{\f");
    push_u32(out, u32::from(font.id));
    out.push(b'\\');
    out.extend_from_slice(font.family.control_word().as_bytes());
    if let Some(cs) = font.charset {
        out.extend_from_slice(br"\fcharset");
        push_u32(out, u32::from(cs));
    }
    // A font name is scanned verbatim by every reader, `\ucN` fallback
    // characters included — so a fallback here would land *in the name*.
    // `\uc0` inside the entry group says there is none, and the group's closing
    // brace restores whatever the document declared.
    let ascii = font.name.is_ascii();
    if !ascii {
        out.extend_from_slice(br"\uc0");
    }
    out.push(b' ');
    escape_name(out, font.name);
    out.extend_from_slice(b";}");
}

fn write_props(out: &mut Vec<u8>, props: &CharProps) {
    if let Some(f) = props.font {
        out.extend_from_slice(br"\f");
        push_u32(out, u32::from(f));
    }
    // Always stated, never inherited. `\plain` resets the size to the reader's
    // default and readers disagree about what that is.
    out.extend_from_slice(br"\fs");
    push_u32(out, u32::from(props.size_half_points));
    if props.bold {
        out.extend_from_slice(br"\b");
    }
    if props.italic {
        out.extend_from_slice(br"\i");
    }
    if props.underline {
        out.extend_from_slice(br"\ul");
    }
    if props.strike {
        out.extend_from_slice(br"\strike");
    }
    if let Some(i) = props.foreground {
        out.extend_from_slice(br"\cf");
        push_u32(out, u32::from(i));
    }
    if let Some(i) = props.background {
        // Both spellings, because the two readers that matter disagree. AppKit
        // honours `\cb` and ignores `\highlight`; Word writes `\highlight` and
        // RichEdit reads it. Verified against `NSAttributedString(rtf:)`:
        // `\highlight1` alone produces no background attribute at all.
        out.extend_from_slice(br"\cb");
        push_u32(out, u32::from(i));
        out.extend_from_slice(br"\highlight");
        push_u32(out, u32::from(i));
    }
}

/// Escape text into the body of a document.
fn escape_text(out: &mut Vec<u8>, s: &str) {
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend_from_slice(br"\\"),
            '{' => out.extend_from_slice(br"\{"),
            '}' => out.extend_from_slice(br"\}"),
            // A paragraph break. `\line` would be a soft break *inside* one,
            // and neither this crate's `Document` nor the facade's `RichText`
            // distinguishes the two, so the one a receiver turns back into a
            // newline is the right choice.
            '\n' => out.extend_from_slice(br"\par "),
            '\r' => {
                // CRLF is one break, not two. Windows plain text arrives this
                // way and doubling every paragraph is a visible bug.
                let mut probe = chars.clone();
                if probe.next() == Some('\n') {
                    chars = probe;
                }
                out.extend_from_slice(br"\par ");
            }
            '\t' => out.extend_from_slice(br"\tab "),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c as u8),
            c => escape_unicode(out, c, true),
        }
    }
}

/// Escape a `\fonttbl` name.
///
/// `;` terminates an entry, so it has to leave as an escape rather than as
/// itself — `\'3b` is a code-page byte, but 0x3B is ASCII in every code page
/// there is, so this is the one hex escape that cannot be misread.
fn escape_name(out: &mut Vec<u8>, s: &str) {
    for c in s.chars() {
        match c {
            '\\' => out.extend_from_slice(br"\\"),
            '{' => out.extend_from_slice(br"\{"),
            '}' => out.extend_from_slice(br"\}"),
            ';' => out.extend_from_slice(br"\'3b"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c as u8),
            // No fallback character: the entry declared `\uc0`.
            c => escape_unicode(out, c, false),
        }
    }
}

/// Write `c` as one or two `\uN` escapes.
///
/// The parameter is signed 16-bit per the spec, so a code unit above 32767 —
/// every surrogate among them — is written as its negative wraparound. A
/// character outside the BMP is two escapes, and under `\uc1` that means two
/// fallback characters for one character of text; that is what Word does and
/// what the counter is defined to mean.
fn escape_unicode(out: &mut Vec<u8>, c: char, fallback: bool) {
    let mut buf = [0u16; 2];
    for unit in c.encode_utf16(&mut buf) {
        out.extend_from_slice(br"\u");
        push_i32(out, i32::from(*unit as i16));
        if fallback {
            let f = ascii_fallback(c);
            // A space directly after a control word's *parameter* is the
            // delimiter and is consumed by the tokenizer, so a fallback space
            // never reaches the `\ucN` counter -- which then skips whatever
            // follows instead, and for `\u160 \u8211-` that is the whole en
            // dash. Writing the delimiter explicitly puts the fallback space
            // back where the counter can see it. Found by the
            // `rtf_write_round_trip` fuzz target.
            if f == b' ' {
                out.push(b' ');
            }
            out.push(f);
        } else {
            // Without a fallback character the escape still needs terminating,
            // or `\u233` followed by the name's next digit reads as `⌱`.
            // A space is the delimiter and is consumed, so a literal space that
            // follows in the name survives as a second one.
            out.push(b' ');
        }
    }
}

/// The single ASCII character that stands in for `c` in a reader that does not
/// implement `\uN`.
///
/// Exactly one character, always: `\uc1` is in the header, and a two-character
/// fallback under `\uc1` makes a conforming reader eat a character of real
/// text. That rules out the obvious `...` for an ellipsis, and is why this
/// table is as short as it is.
fn ascii_fallback(c: char) -> u8 {
    match c {
        '\u{00A0}' | '\u{2007}' | '\u{202F}' => b' ',
        '\u{00AD}' | '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}'
        | '\u{2015}' => b'-',
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => b'\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => b'"',
        _ => b'?',
    }
}

fn push_u32(out: &mut Vec<u8>, n: u32) {
    // u32::MAX is ten digits. Sized in `u32`s rather than in `usize`s on
    // purpose: this crate builds for 32-bit targets, where a `usize` literal
    // wide enough for a 64-bit value does not exist.
    let mut buf = [0u8; 10];
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

fn push_i32(out: &mut Vec<u8>, n: i32) {
    if n < 0 {
        out.push(b'-');
    }
    push_u32(out, n.unsigned_abs());
}

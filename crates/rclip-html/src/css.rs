//! Just enough CSS to read a `style=` attribute.
//!
//! A declaration splitter and seven value parsers. **Not a CSS parser**: there
//! are no selectors, no cascade, no specificity, no `!important` ordering, no
//! custom properties, no shorthand expansion beyond taking the colour out of
//! `background`. A `<style>` block's rules are not applied to anything — they
//! are dropped, because applying them would need a selector engine and a tree,
//! and this crate has neither.
//!
//! What that costs is real and worth stating: a fragment that styles its text
//! through a class rather than through `style=` arrives unstyled. Browsers put
//! the inline styles on the elements when they write a clipboard fragment
//! precisely so that the receiving application does not need a cascade, which is
//! why this is a defensible floor rather than a bug.

use rclip_core::Reader;

use crate::style::Color;

/// One `property: value` pair out of a declaration block.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Declaration<'a> {
    /// The property name, trimmed. Compare case-insensitively.
    pub name: &'a str,
    /// The value, trimmed, undecoded.
    pub value: &'a [u8],
}

impl Declaration<'_> {
    /// `true` if this declares the named property, ASCII-case-insensitively.
    #[must_use]
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }
}

/// Split a declaration block on `;`.
///
/// Semicolons inside quotes and inside parentheses are not separators —
/// `font-family: 'Foo; Bar'` is one declaration and `background: url(a;b)` is
/// another — and a declaration with no `:` is dropped rather than guessed at.
#[must_use]
pub fn declarations(block: &[u8]) -> Declarations<'_> {
    Declarations {
        r: Reader::new(block),
    }
}

/// Iterator over a declaration block. See [`declarations`].
#[derive(Debug, Clone)]
pub struct Declarations<'a> {
    r: Reader<'a>,
}

impl<'a> Iterator for Declarations<'a> {
    type Item = Declaration<'a>;

    fn next(&mut self) -> Option<Declaration<'a>> {
        loop {
            if self.r.remaining_len() == 0 {
                return None;
            }
            let rest = self.r.remaining();
            let len = split_at_top_level(rest, b';');
            let decl = self.r.take(len).ok()?;
            // The separator, if this was not the last declaration.
            let _ = self.r.skip(1);

            let Some(colon) = position_at_top_level(decl, b':') else {
                continue;
            };
            let (name, value) = (decl.get(..colon)?, decl.get(colon + 1..)?);
            let name = trim(name);
            if name.is_empty() {
                continue;
            }
            return Some(Declaration {
                name: core::str::from_utf8(name).unwrap_or(""),
                value: trim(value),
            });
        }
    }
}

/// How far to the next unquoted, unparenthesised `sep`.
fn split_at_top_level(rest: &[u8], sep: u8) -> usize {
    position_at_top_level(rest, sep).unwrap_or(rest.len())
}

fn position_at_top_level(rest: &[u8], sep: u8) -> Option<usize> {
    let mut quote: Option<u8> = None;
    let mut depth: u32 = 0;
    let mut i = 0;
    while let Some(&b) = rest.get(i) {
        // A character reference is one unit. `style="font-family:&quot;Foo
        // Bar&quot;"` is one declaration, and a splitter that treated the `;`
        // of `&quot;` as a separator would cut it into three — which is what
        // this crate's own HTML writer produces, so the bug would be
        // self-inflicted as well as common.
        if quote.is_none() && b == b'&' {
            if let Some(reference) = crate::entity::decode(rest, i) {
                i += reference.len;
                continue;
            }
        }
        match (quote, b) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'(') => depth = depth.saturating_add(1),
            (None, b')') => depth = depth.saturating_sub(1),
            (None, c) if c == sep && depth == 0 => return Some(i),
            (None, _) => {}
        }
        i += 1;
    }
    None
}

/// Trim ASCII whitespace from both ends.
#[must_use]
pub fn trim(mut b: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = b {
        if first.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = b {
        if last.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    b
}

/// Strip one layer of CSS string quotes, entity-escaped ones included.
///
/// `style="font-family:&quot;Foo Bar&quot;"` is what a serializer writes when
/// the attribute is already double-quoted, and this crate's own HTML writer is
/// one of the things that writes it. Stripping only literal quotes would leave
/// a font named `&quot;Foo Bar&quot;`.
#[must_use]
pub fn unquote(value: &[u8]) -> &[u8] {
    const PAIRS: [(&[u8], &[u8]); 6] = [
        (b"\"", b"\""),
        (b"'", b"'"),
        (b"&quot;", b"&quot;"),
        (b"&#34;", b"&#34;"),
        (b"&apos;", b"&apos;"),
        (b"&#39;", b"&#39;"),
    ];
    for (open, close) in PAIRS {
        if value.len() > open.len() + close.len() - 1
            && value.starts_with(open)
            && value.ends_with(close)
        {
            if let Some(inner) = value.get(open.len()..value.len() - close.len()) {
                return trim(inner);
            }
        }
    }
    value
}

// ------------------------------------------------------------------ values

/// `font-weight`. `None` for a value that says nothing.
#[must_use]
pub fn font_weight_bold(value: &[u8]) -> Option<bool> {
    let v = trim(value);
    if v.eq_ignore_ascii_case(b"bold") || v.eq_ignore_ascii_case(b"bolder") {
        return Some(true);
    }
    if v.eq_ignore_ascii_case(b"normal") || v.eq_ignore_ascii_case(b"lighter") {
        return Some(false);
    }
    // The numeric scale. 600 is the boundary every renderer draws, and it is
    // where `<b>` maps to (700).
    let (n, used) = number(v)?;
    if used != v.len() {
        return None;
    }
    Some(n >= 600.0)
}

/// `font-style`.
#[must_use]
pub fn font_style_italic(value: &[u8]) -> Option<bool> {
    let v = trim(value);
    if v.eq_ignore_ascii_case(b"italic") || v.eq_ignore_ascii_case(b"oblique") {
        return Some(true);
    }
    if v.eq_ignore_ascii_case(b"normal") {
        return Some(false);
    }
    None
}

/// What a `text-decoration` / `text-decoration-line` value asks for.
///
/// A single declaration, so it *replaces* rather than adds: `text-decoration:
/// underline` on an element inside a struck-through one turns the strike off
/// for that element, which is what CSS says and what a reader that ORed the two
/// together would get wrong.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct Decoration {
    /// `underline` appeared.
    pub underline: bool,
    /// `line-through` appeared.
    pub strike: bool,
}

/// Parse `text-decoration`. `None` if the value names neither line.
#[must_use]
pub fn text_decoration(value: &[u8]) -> Option<Decoration> {
    let mut out = Decoration::default();
    let mut saw_none = false;
    for word in value.split(u8::is_ascii_whitespace) {
        if word.eq_ignore_ascii_case(b"underline") {
            out.underline = true;
        } else if word.eq_ignore_ascii_case(b"line-through") {
            out.strike = true;
        } else if word.eq_ignore_ascii_case(b"none") {
            saw_none = true;
        }
    }
    if out.underline || out.strike || saw_none {
        Some(out)
    } else {
        None
    }
}

/// A colour value, which has three outcomes rather than two.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ColorValue {
    /// A colour.
    Rgb(Color),
    /// `transparent`, or an `rgba()` with zero alpha. Distinct from "no value":
    /// a transparent background is an instruction to *clear* an inherited one,
    /// and folding it to black is how a highlight appears where none was asked
    /// for.
    Transparent,
}

/// Parse a colour: `#rgb`, `#rrggbb`, `#rgba`, `#rrggbbaa`, `rgb()`, `rgba()`,
/// or a name from [`NAMED_COLORS`].
///
/// Alpha is read only far enough to tell fully transparent from anything else,
/// because neither RTF's `\colortbl` nor this workspace's `Rgb` carries one.
#[must_use]
pub fn color(value: &[u8]) -> Option<ColorValue> {
    let v = trim(value);
    if v.is_empty() {
        return None;
    }
    if v.eq_ignore_ascii_case(b"transparent") {
        return Some(ColorValue::Transparent);
    }
    if let Some(hex) = v.strip_prefix(b"#") {
        return hex_color(hex);
    }
    if let Some(open) = v.iter().position(|b| *b == b'(') {
        let func = v.get(..open)?;
        if func.eq_ignore_ascii_case(b"rgb") || func.eq_ignore_ascii_case(b"rgba") {
            let args = v.get(open + 1..v.len().saturating_sub(1))?;
            return rgb_function(args);
        }
        return None;
    }
    named_color(v).map(ColorValue::Rgb)
}

fn hex_color(hex: &[u8]) -> Option<ColorValue> {
    let d = |i: usize| -> Option<u8> { hex.get(i).copied().and_then(hex_digit) };
    match hex.len() {
        3 | 4 => {
            let (r, g, b) = (d(0)?, d(1)?, d(2)?);
            if hex.len() == 4 && d(3)? == 0 {
                return Some(ColorValue::Transparent);
            }
            Some(ColorValue::Rgb(Color::new(r * 17, g * 17, b * 17)))
        }
        6 | 8 => {
            let byte = |i: usize| -> Option<u8> { Some(d(i)? << 4 | d(i + 1)?) };
            let (r, g, b) = (byte(0)?, byte(2)?, byte(4)?);
            if hex.len() == 8 && byte(6)? == 0 {
                return Some(ColorValue::Transparent);
            }
            Some(ColorValue::Rgb(Color::new(r, g, b)))
        }
        _ => None,
    }
}

/// `rgb(1, 2, 3)`, `rgb(1 2 3 / 50%)`, `rgba(1,2,3,0)`.
fn rgb_function(args: &[u8]) -> Option<ColorValue> {
    let mut parts = [0f32; 4];
    let mut n = 0;
    let mut rest = args;
    while n < 4 {
        let t = trim(rest);
        if t.is_empty() {
            break;
        }
        let (v, used) = number(t)?;
        // A percentage component is 0..100 of 255.
        let scaled = if t.get(used) == Some(&b'%') {
            v * 255.0 / 100.0
        } else {
            v
        };
        parts[n] = scaled;
        n += 1;
        let after = t.get(used..).unwrap_or_default();
        let after = after.strip_prefix(b"%").unwrap_or(after);
        let after = trim(after);
        rest = after
            .strip_prefix(b",")
            .or_else(|| after.strip_prefix(b"/"))
            .unwrap_or(after);
        if rest.is_empty() {
            break;
        }
    }
    if n < 3 {
        return None;
    }
    // The alpha component is 0..1, or a percentage that `scaled` already blew
    // up by 255; either way only "exactly zero" is acted on.
    if n == 4 && parts[3] == 0.0 {
        return Some(ColorValue::Transparent);
    }
    Some(ColorValue::Rgb(Color::new(
        clamp_u8(parts[0]),
        clamp_u8(parts[1]),
        clamp_u8(parts[2]),
    )))
}

fn clamp_u8(v: f32) -> u8 {
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= 255.0 {
        255
    } else {
        // Round to nearest; `f32::round` is in `std`.
        (v + 0.5) as u8
    }
}

const fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Look up a CSS colour keyword.
#[must_use]
pub fn named_color(name: &[u8]) -> Option<Color> {
    let name = core::str::from_utf8(name).ok()?;
    NAMED_COLORS
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, c)| *c)
}

/// `font-size`, in points.
///
/// `parent_pt` is what `em`, `%`, `smaller` and `larger` are relative to. `None`
/// means the reader's body size, which this treats as 12pt — the same number
/// RTF's `\fs24` default and CSS's `medium` (16px) both land on.
#[must_use]
pub fn font_size_pt(value: &[u8], parent_pt: Option<f32>) -> Option<f32> {
    let v = trim(value);
    let base = parent_pt.unwrap_or(DEFAULT_PT);
    // The absolute-size keywords, scaled off CSS's `medium` = 16px = 12pt.
    for (name, px) in [
        ("xx-small", 9.0f32),
        ("x-small", 10.0),
        ("small", 13.0),
        ("medium", 16.0),
        ("large", 18.0),
        ("x-large", 24.0),
        ("xx-large", 32.0),
    ] {
        if v.eq_ignore_ascii_case(name.as_bytes()) {
            return Some(px * PX_TO_PT);
        }
    }
    if v.eq_ignore_ascii_case(b"smaller") {
        return finite(base / 1.2);
    }
    if v.eq_ignore_ascii_case(b"larger") {
        return finite(base * 1.2);
    }

    let (n, used) = number(v)?;
    let unit = trim(v.get(used..)?);
    let pt = if unit.eq_ignore_ascii_case(b"pt") {
        n
    } else if unit.eq_ignore_ascii_case(b"px") {
        n * PX_TO_PT
    } else if unit.eq_ignore_ascii_case(b"pc") {
        n * 12.0
    } else if unit.eq_ignore_ascii_case(b"in") {
        n * 72.0
    } else if unit.eq_ignore_ascii_case(b"cm") {
        n * 72.0 / 2.54
    } else if unit.eq_ignore_ascii_case(b"mm") {
        n * 72.0 / 25.4
    } else if unit.eq_ignore_ascii_case(b"q") {
        n * 72.0 / 101.6
    } else if unit.eq_ignore_ascii_case(b"em") || unit.eq_ignore_ascii_case(b"rem") {
        n * base
    } else if unit.eq_ignore_ascii_case(b"ex") || unit.eq_ignore_ascii_case(b"ch") {
        n * base / 2.0
    } else if unit == b"%" {
        n * base / 100.0
    } else if unit.is_empty() {
        // A bare number is not a valid CSS length. It is, however, what a
        // `<font size>` attribute holds, and that has its own entry point.
        return None;
    } else {
        return None;
    };
    finite(pt)
}

/// The legacy `<font size="N">` scale, in points.
///
/// `+N` and `-N` are relative to size 3, which is the HTML default and is
/// `medium`.
#[must_use]
pub fn font_attr_size_pt(value: &[u8]) -> Option<f32> {
    const SCALE_PX: [f32; 7] = [10.0, 13.0, 16.0, 18.0, 24.0, 32.0, 48.0];
    let v = trim(value);
    let relative = matches!(v.first(), Some(b'+' | b'-'));
    let (n, used) = number(v)?;
    if used == 0 {
        return None;
    }
    let index = if relative { 3.0 + n } else { n };
    let i = if index.is_nan() {
        return None;
    } else if index < 1.0 {
        0usize
    } else if index > 7.0 {
        6usize
    } else {
        (index as usize).saturating_sub(1)
    };
    Some(SCALE_PX[i] * PX_TO_PT)
}

/// CSS `medium`, RTF's `\fs24`, and every renderer's body default.
pub const DEFAULT_PT: f32 = 12.0;

/// 96 CSS pixels to the inch, 72 points to the inch.
const PX_TO_PT: f32 = 0.75;

fn finite(v: f32) -> Option<f32> {
    (v.is_finite() && v > 0.0).then_some(v)
}

/// Parse a leading number. Returns the value and how many bytes it used.
///
/// Hand-rolled because `str::parse::<f32>` is in `core` but pulls in the full
/// decimal parser, and because a CSS value's number is always short and always
/// followed by something that is not a digit.
fn number(v: &[u8]) -> Option<(f32, usize)> {
    let mut i = 0;
    let negative = match v.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let start = i;
    let mut whole: f32 = 0.0;
    while let Some(d) = v.get(i).and_then(|b| b.is_ascii_digit().then(|| b - b'0')) {
        whole = whole * 10.0 + f32::from(d);
        i += 1;
    }
    let mut frac: f32 = 0.0;
    let mut scale: f32 = 1.0;
    if v.get(i) == Some(&b'.') {
        i += 1;
        while let Some(d) = v.get(i).and_then(|b| b.is_ascii_digit().then(|| b - b'0')) {
            scale /= 10.0;
            frac += f32::from(d) * scale;
            i += 1;
        }
    }
    if i == start || (i == start + 1 && v.get(start) == Some(&b'.')) {
        return None;
    }
    let value = whole + frac;
    Some((if negative { -value } else { value }, i))
}

/// The colour keywords this crate resolves.
///
/// The CSS named-colour list is 148 entries; this is the HTML 4 sixteen plus the
/// greys and the handful of names that turn up in real markup, because a table
/// of every X11 colour name would be most of the crate's rodata for the benefit
/// of `papayawhip`. Anything unlisted is treated as "no colour stated", which
/// inherits — visibly nothing, rather than visibly wrong.
pub static NAMED_COLORS: &[(&str, Color)] = &[
    ("aqua", Color::new(0, 255, 255)),
    ("aquamarine", Color::new(127, 255, 212)),
    ("beige", Color::new(245, 245, 220)),
    ("black", Color::new(0, 0, 0)),
    ("blue", Color::new(0, 0, 255)),
    ("brown", Color::new(165, 42, 42)),
    ("crimson", Color::new(220, 20, 60)),
    ("cyan", Color::new(0, 255, 255)),
    ("darkblue", Color::new(0, 0, 139)),
    ("darkgray", Color::new(169, 169, 169)),
    ("darkgreen", Color::new(0, 100, 0)),
    ("darkgrey", Color::new(169, 169, 169)),
    ("darkred", Color::new(139, 0, 0)),
    ("fuchsia", Color::new(255, 0, 255)),
    ("gold", Color::new(255, 215, 0)),
    ("gray", Color::new(128, 128, 128)),
    ("green", Color::new(0, 128, 0)),
    ("grey", Color::new(128, 128, 128)),
    ("indigo", Color::new(75, 0, 130)),
    ("lightblue", Color::new(173, 216, 230)),
    ("lightgray", Color::new(211, 211, 211)),
    ("lightgreen", Color::new(144, 238, 144)),
    ("lightgrey", Color::new(211, 211, 211)),
    ("lightyellow", Color::new(255, 255, 224)),
    ("lime", Color::new(0, 255, 0)),
    ("magenta", Color::new(255, 0, 255)),
    ("maroon", Color::new(128, 0, 0)),
    ("navy", Color::new(0, 0, 128)),
    ("olive", Color::new(128, 128, 0)),
    ("orange", Color::new(255, 165, 0)),
    ("pink", Color::new(255, 192, 203)),
    ("purple", Color::new(128, 0, 128)),
    ("red", Color::new(255, 0, 0)),
    ("silver", Color::new(192, 192, 192)),
    ("teal", Color::new(0, 128, 128)),
    ("violet", Color::new(238, 130, 238)),
    ("white", Color::new(255, 255, 255)),
    ("yellow", Color::new(255, 255, 0)),
];

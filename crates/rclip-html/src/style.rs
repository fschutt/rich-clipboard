//! The character formatting an inline element can carry.

use crate::text::HtmlText;

/// An 8-bit sRGB colour.
///
/// No alpha. CSS has one and neither RTF's `\colortbl` nor this workspace's
/// `RichText` does, so carrying it here would be carrying a field that is
/// dropped at the first conversion — and a field that is always opaque is a
/// field that will eventually be believed. `rgba(0,0,0,0)` is reported as
/// [`crate::css::ColorValue::Transparent`] instead, which is information the
/// caller can act on.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Color {
    /// Construct a colour.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// The formatting in effect for a run of text.
///
/// Every field is an *override*: `None` and `false` mean "whatever the
/// receiving application uses for body text", not black 12pt on white. A run
/// that names no colour should inherit one, and a run that names black should
/// get black even on a dark background.
///
/// `Copy`, because the element stack keeps one per open element in a fixed-size
/// array and `<b>` / `</b>` are an assignment each.
#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub struct Style<'a> {
    /// `<b>`, `<strong>`, `font-weight` of 600 or more.
    pub bold: bool,
    /// `<i>`, `<em>`, `font-style: italic`.
    pub italic: bool,
    /// `<u>`, `<ins>`, `text-decoration: underline`.
    pub underline: bool,
    /// `<s>`, `<strike>`, `<del>`, `text-decoration: line-through`.
    pub strike: bool,
    /// `font-size`, resolved to points against the enclosing element's size.
    /// `None` inherits.
    pub size_pt: Option<f32>,
    /// The first family named by `font-family`, or `<font face>`, quotes
    /// stripped. Not resolved against any font on this machine, because there
    /// is no machine here.
    pub font_family: Option<HtmlText<'a>>,
    /// `color`, or `<font color>`. `None` inherits.
    pub color: Option<Color>,
    /// `background-color`, the colour out of a `background` shorthand, or
    /// `bgcolor`. `None` is no background — which is what `transparent`
    /// resolves to as well.
    pub background: Option<Color>,
}

impl Style<'_> {
    /// What the document starts with, and what an element inherits when nothing
    /// above it said anything.
    ///
    /// A `const` rather than only a `Default` impl because the element stack is
    /// a fixed-size array that has to be initialised in a `const` context.
    pub const DEFAULT: Style<'static> = Style {
        bold: false,
        italic: false,
        underline: false,
        strike: false,
        size_pt: None,
        font_family: None,
        color: None,
        background: None,
    };

    /// `true` if this run overrides nothing.
    #[must_use]
    pub fn is_default(&self) -> bool {
        !self.bold
            && !self.italic
            && !self.underline
            && !self.strike
            && self.size_pt.is_none()
            && self.font_family.is_none()
            && self.color.is_none()
            && self.background.is_none()
    }
}

//! The owned representation, behind the `alloc` feature.
//!
//! This exists for the same reason `rclip-rtf`'s does: `&amp;` is one character
//! written as five bytes and a run of indentation is one space, so the decoded
//! text of an HTML fragment is **not** a contiguous slice of its bytes
//! anywhere. A borrowing API cannot hand back `"a & b"` when the input says
//! `a &amp; b`.
//!
//! Everything else in the crate works without this. A caller that can consume
//! [`crate::Run`]s as they arrive should, and stay allocation-free.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use rclip_core::Result;

use crate::parse::{RunText, Runs};
use crate::style::{Color, Style};

/// [`Style`] with its font name owned.
///
/// The borrowing [`Style`] keeps the font family as a view into the input,
/// because a `style=` attribute's value needs entity decoding before it is a
/// name. Once the text is owned there is nothing left to borrow from.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OwnedStyle {
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underlined.
    pub underline: bool,
    /// Struck through.
    pub strike: bool,
    /// Size in points. `None` inherits.
    pub size_pt: Option<f32>,
    /// The first family named by `font-family` or `<font face>`.
    pub font_family: Option<String>,
    /// Text colour.
    pub color: Option<Color>,
    /// Background colour.
    pub background: Option<Color>,
}

impl OwnedStyle {
    /// `true` if this run overrides nothing.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    fn from_borrowed(style: &Style<'_>) -> Self {
        Self {
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            strike: style.strike,
            size_pt: style.size_pt,
            font_family: style
                .font_family
                .map(|f| f.chars().collect::<String>())
                .filter(|f| !f.is_empty()),
            color: style.color,
            background: style.background,
        }
    }
}

/// A styled range of [`Document::text`].
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// Byte range into [`Document::text`]. Always on character boundaries.
    pub range: Range<usize>,
    /// The formatting over that range.
    pub style: OwnedStyle,
}

/// A parsed HTML fragment: one string of text, and the formatting spans over it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    /// The text, with breaks as `\n` and cell boundaries as `\t`.
    pub text: String,
    /// Styled ranges, in order, covering `text` with no gaps and no overlaps.
    pub runs: Vec<Run>,
}

impl Document {
    /// Parse a fragment.
    ///
    /// # Errors
    ///
    /// [`rclip_core::ErrorKind::DepthLimit`] and nothing else. Every other
    /// malformation — a mismatched end tag, an unterminated attribute, a `<`
    /// that begins nothing, invalid UTF-8 — is absorbed, because a clipboard
    /// paste that drops one character beats a clipboard paste that fails, and
    /// because malformed clipboard HTML is the normal case rather than the
    /// exception.
    pub fn parse(input: &[u8]) -> Result<Self> {
        let mut doc = Self::default();
        // The style of the run being accumulated, and where it started.
        let mut open: Option<(OwnedStyle, usize)> = None;
        for run in Runs::new(input) {
            let run = run?;
            let start = doc.text.len();
            match run.text {
                RunText::Text(t) => match t.as_str() {
                    Some(s) => doc.text.push_str(s),
                    None => doc.text.extend(t.chars()),
                },
                RunText::Break => doc.text.push('\n'),
                RunText::Tab => doc.text.push('\t'),
            }
            if doc.text.len() == start {
                continue;
            }
            let style = OwnedStyle::from_borrowed(&run.style);
            match open {
                // Merge: an element boundary cuts a run even when the
                // formatting either side of it is identical, so an unmerged
                // document has one run per tag.
                Some((ref props, _)) if *props == style => {}
                Some((props, from)) => {
                    doc.runs.push(Run {
                        range: from..start,
                        style: props,
                    });
                    open = Some((style, start));
                }
                None => open = Some((style, start)),
            }
        }
        if let Some((style, from)) = open {
            doc.runs.push(Run {
                range: from..doc.text.len(),
                style,
            });
        }
        doc.trim_trailing();
        Ok(doc)
    }

    /// Drop a trailing space, which a fragment ending in whitespace before its
    /// closing tag produces and which no renderer shows.
    fn trim_trailing(&mut self) {
        while self.text.ends_with(' ') {
            self.text.pop();
            if let Some(last) = self.runs.last_mut() {
                last.range.end = self.text.len();
                if last.range.start >= last.range.end {
                    self.runs.pop();
                }
            }
        }
    }

    /// The text of one run.
    ///
    /// Empty rather than panicking if `run` came from a different `Document`.
    #[must_use]
    pub fn run_text(&self, run: &Run) -> &str {
        self.text.get(run.range.clone()).unwrap_or_default()
    }

    /// `true` if nothing is styled.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        self.runs.iter().all(|r| r.style.is_default())
    }
}

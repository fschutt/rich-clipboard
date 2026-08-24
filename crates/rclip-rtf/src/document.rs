//! The owned representation, behind the `alloc` feature.
//!
//! This module exists for exactly one reason: `\uN` and `\'hh` mean the decoded
//! text of an RTF document is **not** a contiguous slice of its bytes. A
//! borrowing API cannot hand back `"café"` when the input says `caf\'e9`, so
//! anything that wants one string has to own it.
//!
//! Everything else in the crate works without this. If a caller can consume
//! [`crate::StyledRun`]s as they arrive — writing into a text layout engine, or
//! into a buffer it already owns — it should, and stay allocation-free.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use rclip_core::Result;

use crate::codepage::Codepage;
use crate::parse::{header, Parser, RunText};
use crate::style::{CharProps, Color, FontFamily};
use crate::tables;

/// A styled range of [`Document::text`].
///
/// A range rather than a `String` so the plain text stays one contiguous
/// buffer: that is the form `CF_UNICODETEXT` and `public.utf8-plain-text` want,
/// and duplicating the text into per-run `String`s would mean rebuilding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Byte range into [`Document::text`].
    pub range: Range<usize>,
    pub props: CharProps,
}

/// A `\fonttbl` entry with its name decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedFont {
    /// The `N` of `\fN` — a key, not a position.
    pub id: u16,
    pub family: FontFamily,
    pub charset: Option<u16>,
    pub name: String,
}

/// A parsed RTF document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    /// The body text, with `\par` and `\line` as `\n`.
    pub text: String,
    /// Styled ranges, in order, covering `text` with no gaps and no overlaps.
    pub runs: Vec<Run>,
    pub fonts: Vec<OwnedFont>,
    /// `\colortbl`, in declaration order. `None` is an omitted ("auto") entry;
    /// index it with [`CharProps::foreground`] / [`CharProps::background`].
    pub colors: Vec<Option<Color>>,
    /// `{\*\generator ...}`, with the trailing `;` stripped.
    pub generator: Option<String>,
    /// The code page the header declared.
    pub codepage: Codepage,
    /// `\deffN`, the font [`CharProps::font`] means by `None`.
    pub default_font: Option<u16>,
}

impl Document {
    /// Parse a whole document.
    ///
    /// Fails on a bad signature, unbalanced braces, or nesting past
    /// [`rclip_core::MAX_DEPTH`]. Everything recoverable — an unknown control
    /// word, an undecodable byte, a lone surrogate — is absorbed rather than
    /// raised, because a clipboard paste that drops one character beats a
    /// clipboard paste that fails.
    pub fn parse(input: &[u8]) -> Result<Self> {
        let head = header(input)?;
        let mut doc = Self {
            codepage: head.codepage,
            default_font: head.default_font,
            generator: tables::generator(input, head.codepage)
                .map(|g| g.chars().collect::<String>()),
            colors: tables::colors(input).collect(),
            fonts: tables::fonts(input, head.codepage)
                .map(|f| OwnedFont {
                    id: f.id,
                    family: f.family,
                    charset: f.charset,
                    name: f.name.chars().collect(),
                })
                .collect(),
            ..Self::default()
        };

        let mut parser = Parser::new(input)?;
        // `props` of the run currently being accumulated, and where it started.
        let mut open: Option<(CharProps, usize)> = None;
        for run in &mut parser {
            let run = run?;
            let start = doc.text.len();
            match run.text {
                RunText::Text(s) => doc.text.push_str(s),
                RunText::Char(c) => doc.text.push(c),
                // Both breaks flatten to `\n`. The distinction survives in the
                // borrowing API; plain clipboard text has no room for it.
                RunText::ParagraphBreak | RunText::LineBreak => doc.text.push('\n'),
            }
            if doc.text.len() == start {
                continue;
            }
            match open {
                // Merge: the parser cuts runs at group boundaries and at every
                // escape, so an unmerged document has one run per character.
                Some((props, _)) if props == run.props => {}
                Some((props, from)) => {
                    doc.runs.push(Run {
                        range: from..start,
                        props,
                    });
                    open = Some((run.props, start));
                }
                None => open = Some((run.props, start)),
            }
        }
        if let Some((props, from)) = open {
            doc.runs.push(Run {
                range: from..doc.text.len(),
                props,
            });
        }
        doc.default_font = doc.default_font.or_else(|| parser.default_font());
        Ok(doc)
    }

    /// The text of one run.
    #[must_use]
    pub fn run_text(&self, run: &Run) -> &str {
        self.text.get(run.range.clone()).unwrap_or_default()
    }

    /// Look up a `\cfN` / `\cbN` index. `None` means out of range *or* the
    /// "auto" entry; both mean "use the reader's default colour".
    #[must_use]
    pub fn color(&self, index: u16) -> Option<Color> {
        self.colors.get(index as usize).copied().flatten()
    }

    /// Look up a `\fN` index. Font numbers are assigned by the writer and are
    /// not positions, so this is a search, not an index.
    #[must_use]
    pub fn font(&self, id: u16) -> Option<&OwnedFont> {
        self.fonts.iter().find(|f| f.id == id)
    }
}

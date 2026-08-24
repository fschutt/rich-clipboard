//! Character properties, font-table and colour-table entries.

use crate::codepage::Codepage;
use crate::token::{ControlSymbol, Token, Tokenizer};

/// The character formatting in effect for a run of text.
///
/// `Copy`, and deliberately small: the parser keeps one of these per open group
/// in a fixed-size stack, and `{`/`}` save and restore it by assignment.
///
/// Colours and fonts stay as *indices*. Resolving them needs the tables, the
/// tables live elsewhere in the document, and resolving eagerly would either
/// force an allocation or force a two-pass parse. Look them up with
/// [`crate::colors`] and [`crate::fonts`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct CharProps {
    /// `\b` / `\b0`
    pub bold: bool,
    /// `\i` / `\i0`
    pub italic: bool,
    /// `\ul` / `\ulnone`. Every `\ul*` variant collapses to `true`;
    /// `// TODO(phase-1):` keep the style (dotted, wave, double).
    pub underline: bool,
    /// `\strike` / `\strike0`
    pub strike: bool,
    /// `\fsN`, in **half-points**. `\fs24` is 12pt. The RTF default is 24.
    pub size_half_points: u16,
    /// `\fN` index into the font table, or `None` for the document default
    /// (`\deffN`).
    pub font: Option<u16>,
    /// `\cfN` index into the colour table.
    pub foreground: Option<u16>,
    /// `\cbN` or `\highlightN` index into the colour table.
    pub background: Option<u16>,
}

impl Default for CharProps {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl CharProps {
    /// What `\plain` resets to, and what a document starts with.
    ///
    /// A `const` rather than only a `Default` impl because the parser's group
    /// stack is a fixed-size array that has to be initialised in a `const`
    /// context.
    pub const DEFAULT: Self = Self {
        bold: false,
        italic: false,
        underline: false,
        strike: false,
        // The RTF default font size is 24 half-points, i.e. 12pt. A parser
        // that defaults this to 0 reports every unstyled run as invisible.
        size_half_points: 24,
        font: None,
        foreground: None,
        background: None,
    };

    /// Font size in points. `\fs24` is 12.0.
    #[must_use]
    pub fn points(self) -> f32 {
        f32::from(self.size_half_points) / 2.0
    }

    /// `true` if nothing but the defaults is set — what `\plain` produces.
    #[must_use]
    pub fn is_plain(self) -> bool {
        self == Self::default()
    }
}

/// One entry of `\colortbl`, as 8-bit sRGB.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Color {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

/// `\fnil` .. `\fbidi`, the font-family hint in a `\fonttbl` entry.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
pub enum FontFamily {
    /// `\fnil` — unknown or unspecified.
    #[default]
    Nil,
    /// `\froman` — serif proportional.
    Roman,
    /// `\fswiss` — sans-serif proportional.
    Swiss,
    /// `\fmodern` — fixed pitch.
    Modern,
    /// `\fscript` — cursive.
    Script,
    /// `\fdecor` — decorative.
    Decor,
    /// `\ftech` — symbol.
    Tech,
    /// `\fbidi` — bidirectional.
    Bidi,
}

impl FontFamily {
    /// The control word that declares this family in a `\fonttbl` entry,
    /// without its backslash.
    #[must_use]
    pub const fn control_word(self) -> &'static str {
        match self {
            Self::Nil => "fnil",
            Self::Roman => "froman",
            Self::Swiss => "fswiss",
            Self::Modern => "fmodern",
            Self::Script => "fscript",
            Self::Decor => "fdecor",
            Self::Tech => "ftech",
            Self::Bidi => "fbidi",
        }
    }

    pub(crate) fn from_control(name: &str) -> Option<Self> {
        Some(match name {
            "fnil" => Self::Nil,
            "froman" => Self::Roman,
            "fswiss" => Self::Swiss,
            "fmodern" => Self::Modern,
            "fscript" => Self::Script,
            "fdecor" => Self::Decor,
            "ftech" => Self::Tech,
            "fbidi" => Self::Bidi,
            _ => return None,
        })
    }
}

/// One entry of `\fonttbl`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Font<'a> {
    /// The `N` of `\fN`, which is what [`CharProps::font`] refers to. Font
    /// numbers are assigned by the writer and are neither dense nor ordered,
    /// so this is not a position in the table.
    pub id: u16,
    pub family: FontFamily,
    /// `\fcharsetN`, the Windows charset number. Present because a font name
    /// in a non-Latin script is encoded in *that* charset's code page rather
    /// than the document's.
    pub charset: Option<u16>,
    /// The font name, still RTF-encoded. See [`RtfText`].
    pub name: RtfText<'a>,
}

/// A span of RTF source that represents text.
///
/// It is not a `&str`, because a name like `Times New \'92 Roman` decodes to
/// characters that are not contiguous bytes anywhere in the input. Without
/// `alloc` there is nowhere to put the decoded form, so this stays a lazy view:
/// [`RtfText::as_str`] gives you the fast path when the span happens to be
/// plain ASCII (the overwhelmingly common case), and [`RtfText::chars`] always
/// works.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct RtfText<'a> {
    raw: &'a [u8],
    codepage: Codepage,
}

impl<'a> RtfText<'a> {
    pub(crate) const fn new(raw: &'a [u8], codepage: Codepage) -> Self {
        Self { raw, codepage }
    }

    /// The undecoded bytes, exactly as they appear in the input.
    #[must_use]
    pub const fn as_raw(&self) -> &'a [u8] {
        self.raw
    }

    #[must_use]
    pub const fn codepage(&self) -> Codepage {
        self.codepage
    }

    /// The span as a borrowed string, if it contains no escapes at all.
    ///
    /// `None` means the caller must go through [`RtfText::chars`] — it does not
    /// mean the span is invalid.
    #[must_use]
    pub fn as_str(&self) -> Option<&'a str> {
        if self.raw.iter().any(|&b| b == b'\\' || b >= 0x80) {
            return None;
        }
        core::str::from_utf8(self.raw).ok()
    }

    /// `true` if the span decodes to nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chars().next().is_none()
    }

    /// Decode, resolving `\'hh`, `\uN` and the literal escapes.
    ///
    /// Undecodable bytes become U+FFFD. Unlike body text this path does **not**
    /// apply a `\ucN` skip count: table entries are written by the same writer
    /// that wrote the `\uN`, and in practice carry no ANSI fallback. If one
    /// does, the fallback character appears in the name — visible, and better
    /// than silently eating a real character.
    #[must_use]
    pub fn chars(&self) -> RtfChars<'a> {
        RtfChars {
            tok: Tokenizer::new(self.raw),
            codepage: self.codepage,
            text: "",
        }
    }
}

/// Iterator over the decoded characters of an [`RtfText`].
#[derive(Debug, Clone)]
pub struct RtfChars<'a> {
    tok: Tokenizer<'a>,
    codepage: Codepage,
    /// Remainder of the literal run currently being drained.
    text: &'a str,
}

impl Iterator for RtfChars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        loop {
            if let Some(c) = self.text.chars().next() {
                self.text = &self.text[c.len_utf8()..];
                return Some(c);
            }
            match self.tok.next()? {
                Ok(Token::Text(s)) => self.text = s,
                Ok(Token::RawByte(b)) | Ok(Token::ControlSymbol(ControlSymbol::HexByte(b))) => {
                    return Some(self.codepage.decode_lossy(b))
                }
                Ok(Token::ControlSymbol(ControlSymbol::Literal(c))) => return Some(c),
                Ok(Token::ControlSymbol(ControlSymbol::NonBreakingSpace)) => {
                    return Some('\u{00A0}')
                }
                Ok(Token::ControlWord {
                    name: "u",
                    param: Some(p),
                }) => {
                    // Lone surrogates in a font name are not worth a second
                    // decoding state machine; the body-text parser pairs them.
                    if let Some(c) = crate::parse::unicode_escape_char(p) {
                        return Some(c);
                    }
                    return Some(char::REPLACEMENT_CHARACTER);
                }
                // Groups, other control words and errors carry no name text.
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }
}

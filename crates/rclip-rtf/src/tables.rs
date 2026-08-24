//! The header destinations we actually read: `\fonttbl`, `\colortbl`,
//! `\*\generator`.
//!
//! Each is a standalone scanner over the token stream rather than a hook in
//! [`crate::Parser`]. That costs a second pass over the input — which is a few
//! kilobytes for a clipboard payload — and buys two things worth more than the
//! pass: the body parser stays a single-purpose state machine, and the tables
//! come back as *iterators* that borrow, so reading a colour table needs no
//! allocation and no fixed-capacity array that a document could overflow.
//!
//! Colour and font indices are not positions. `\cf3` means "the entry declared
//! as `\f3`/the fourth colour", and writers leave gaps, so callers should look
//! entries up rather than index into a collected list.

use rclip_core::Reader;

use crate::codepage::Codepage;
use crate::style::{Color, Font, FontFamily, RtfText};
use crate::token::{ControlSymbol, Token, Tokenizer};

/// Iterator over `\colortbl` entries, in declaration order.
///
/// Yields `None` for an entry whose definition was omitted — `{\colortbl;...}`.
/// Per the spec every entry is `;`-terminated even when empty, and the first
/// one conventionally has no definition and means "auto" (the reader's default
/// text colour). That is why the item type is `Option<Color>` and not `Color`:
/// collapsing "auto" to black is how black text ends up hard-coded into a
/// document that should have followed the theme.
#[derive(Debug, Clone)]
pub struct ColorTable<'a> {
    tok: Tokenizer<'a>,
    depth: usize,
    table_depth: usize,
    inside: bool,
    done: bool,
    /// Remainder of the text run being scanned for `;`.
    pending: &'a str,
    red: Option<u8>,
    green: Option<u8>,
    blue: Option<u8>,
}

/// Read the `\colortbl` destination.
#[must_use]
pub fn colors(input: &[u8]) -> ColorTable<'_> {
    ColorTable {
        tok: Tokenizer::new(input),
        depth: 0,
        table_depth: 0,
        inside: false,
        done: false,
        pending: "",
        red: None,
        green: None,
        blue: None,
    }
}

impl ColorTable<'_> {
    fn take_entry(&mut self) -> Option<Color> {
        let (r, g, b) = (self.red.take(), self.green.take(), self.blue.take());
        // Any one component present means the entry was defined; a writer that
        // emits `\red255;` and omits green/blue means those are zero.
        if r.is_none() && g.is_none() && b.is_none() {
            None
        } else {
            Some(Color::new(r.unwrap_or(0), g.unwrap_or(0), b.unwrap_or(0)))
        }
    }
}

impl Iterator for ColorTable<'_> {
    type Item = Option<Color>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(i) = self.pending.find(';') {
                self.pending = &self.pending[i + 1..];
                return Some(self.take_entry());
            }
            self.pending = "";
            if self.done {
                return None;
            }

            let Some(Ok(token)) = self.tok.next() else {
                // A lex error or EOF ends the table. An unterminated final
                // entry is dropped rather than guessed at.
                self.done = true;
                return None;
            };
            match token {
                Token::GroupStart => self.depth += 1,
                Token::GroupEnd => {
                    self.depth = self.depth.saturating_sub(1);
                    if self.inside && self.depth < self.table_depth {
                        self.done = true;
                        return None;
                    }
                }
                Token::ControlWord {
                    name: "colortbl", ..
                } if !self.inside => {
                    self.inside = true;
                    self.table_depth = self.depth;
                }
                Token::ControlWord { name, param } if self.inside => {
                    let v = param.map(|p| p.clamp(0, 255) as u8);
                    match name {
                        "red" => self.red = v,
                        "green" => self.green = v,
                        "blue" => self.blue = v,
                        _ => {}
                    }
                }
                Token::Text(s) if self.inside => self.pending = s,
                _ => {}
            }
        }
    }
}

/// Iterator over `\fonttbl` entries.
///
/// Handles both shapes writers use: one sub-group per font
/// (`{\fonttbl{\f0\fswiss Helvetica;}}`, what everything modern emits) and the
/// older flat form (`{\fonttbl\f0\fswiss Helvetica;\f1\fmodern Courier;}`).
#[derive(Debug, Clone)]
pub struct FontTable<'a> {
    tok: Tokenizer<'a>,
    input: Reader<'a>,
    codepage: Codepage,
    depth: usize,
    table_depth: usize,
    inside: bool,
    done: bool,
    pending: &'a str,
    /// Byte offset in the input where the pending text run started.
    pending_at: usize,
    /// Start of the current entry's name span.
    name_start: usize,
    id: Option<u16>,
    family: FontFamily,
    charset: Option<u16>,
}

/// Read the `\fonttbl` destination.
///
/// `codepage` is what `\'hh` inside a font name decodes as; get it from
/// [`crate::header`].
#[must_use]
pub fn fonts(input: &[u8], codepage: Codepage) -> FontTable<'_> {
    FontTable {
        tok: Tokenizer::new(input),
        input: Reader::new(input),
        codepage,
        depth: 0,
        table_depth: 0,
        inside: false,
        done: false,
        pending: "",
        pending_at: 0,
        name_start: 0,
        id: None,
        family: FontFamily::Nil,
        charset: None,
    }
}

impl<'a> FontTable<'a> {
    /// Finish the entry under construction, with its name ending at `end`.
    fn take_entry(&mut self, end: usize) -> Option<Font<'a>> {
        let id = self.id.take()?;
        let raw = self
            .input
            .slice_at(self.name_start, end.saturating_sub(self.name_start))
            .unwrap_or_default();
        let font = Font {
            id,
            family: self.family,
            charset: self.charset,
            name: RtfText::new(trim_ascii(raw), self.codepage),
        };
        self.family = FontFamily::Nil;
        self.charset = None;
        Some(font)
    }
}

impl<'a> Iterator for FontTable<'a> {
    type Item = Font<'a>;

    fn next(&mut self) -> Option<Font<'a>> {
        loop {
            if let Some(i) = self.pending.find(';') {
                let end = self.pending_at + i;
                self.pending = &self.pending[i + 1..];
                self.pending_at = end + 1;
                let entry = self.take_entry(end);
                // The name of the *next* flat-form entry starts after the `;`.
                self.name_start = self.pending_at;
                if entry.is_some() {
                    return entry;
                }
                continue;
            }
            self.pending = "";
            if self.done {
                return None;
            }

            let Some(Ok(token)) = self.tok.next() else {
                self.done = true;
                return None;
            };
            let at = self.tok.token_offset();
            match token {
                Token::GroupStart => {
                    self.depth += 1;
                    if self.inside && self.depth == self.table_depth + 1 {
                        // A per-font entry group opens; the name starts here.
                        self.id = None;
                        self.family = FontFamily::Nil;
                        self.charset = None;
                        self.name_start = self.tok.pos();
                    }
                }
                Token::GroupEnd => {
                    self.depth = self.depth.saturating_sub(1);
                    if !self.inside {
                        continue;
                    }
                    if self.depth < self.table_depth {
                        self.done = true;
                        return None;
                    }
                    if self.depth == self.table_depth {
                        // Entry group closed; `;` may have been omitted.
                        if let Some(font) = self.take_entry(at) {
                            return Some(font);
                        }
                    } else {
                        // A nested `{\*\panose ...}` or `{\*\falt ...}` closed.
                        // Its bytes are not part of the name.
                        self.name_start = self.tok.pos();
                    }
                }
                Token::ControlWord {
                    name: "fonttbl", ..
                } if !self.inside => {
                    self.inside = true;
                    self.table_depth = self.depth;
                    self.name_start = self.tok.pos();
                }
                Token::ControlWord { name, param } if self.inside => {
                    match name {
                        "f" => self.id = param.map(|p| p.clamp(0, 0xFFFF) as u16),
                        "fcharset" => self.charset = param.map(|p| p.clamp(0, 0xFFFF) as u16),
                        // `\uN` is *part of* the name, not a keyword in front
                        // of it: a font whose name has a character outside the
                        // document's code page is written `{\f1\fnil\uc0
                        // Ma\u241 ana;}`, and cutting the span at the escape
                        // would leave the name as `ana`. `RtfText::chars`
                        // decodes it.
                        "u" => continue,
                        _ => {
                            if let Some(fam) = FontFamily::from_control(name) {
                                self.family = fam;
                            }
                        }
                    }
                    // Whatever the control word was, the name cannot have
                    // started before it ended.
                    self.name_start = self.tok.pos();
                }
                Token::ControlSymbol(ControlSymbol::Ignorable) if self.inside => {
                    self.name_start = self.tok.pos();
                }
                Token::Text(s) if self.inside => {
                    self.pending = s;
                    self.pending_at = at;
                }
                _ => {}
            }
        }
    }
}

/// The `{\*\generator ...}` string, if present.
///
/// Word writes `Riched20 10.0.19041;`, TextEdit writes `Cocoa HTML Writer` or
/// nothing. Knowing the writer is what lets a caller work around a specific
/// application's quirks, so it is worth reading even though it is not content.
/// The trailing `;` is stripped.
#[must_use]
pub fn generator(input: &[u8], codepage: Codepage) -> Option<RtfText<'_>> {
    let mut tok = Tokenizer::new(input);
    let mut start = None;
    while let Some(Ok(token)) = tok.next() {
        match token {
            // A nested group inside `\generator` would break the span. No
            // writer emits one; bail rather than return a half-name.
            Token::GroupStart if start.is_some() => return None,
            Token::GroupEnd => {
                let Some(s) = start else { continue };
                let end = tok.token_offset();
                let raw = Reader::new(input).slice_at(s, end.saturating_sub(s)).ok()?;
                let raw = trim_ascii(raw);
                let raw = raw.strip_suffix(b";").unwrap_or(raw);
                return Some(RtfText::new(trim_ascii(raw), codepage));
            }
            Token::ControlWord {
                name: "generator", ..
            } => start = Some(tok.pos()),
            _ => {}
        }
    }
    None
}

/// `[u8]::trim_ascii` is unstable on the crate's MSRV, and only spaces and tabs
/// can appear here anyway — the lexer already dropped CR and LF.
fn trim_ascii(mut b: &[u8]) -> &[u8] {
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

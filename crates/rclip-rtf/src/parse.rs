//! The group / destination state machine.
//!
//! This is where RTF stops being a token stream and starts being a document.
//! Three rules do almost all of the work, and getting any of them wrong is what
//! makes an RTF reader mangle Word's output:
//!
//! 1. **`{` and `}` save and restore state.** Character properties *and* the
//!    `\ucN` skip count *and* the current destination are per-group. A `\b`
//!    inside braces does not survive the closing brace.
//! 2. **`{\*\unknown ...}` is dropped wholesale, nested groups included** —
//!    but `{\unknown ...}` without the `\*` is not: the control word is ignored
//!    and the group's text is still document text. Collapsing those two cases
//!    either eats real content or spills `HYPERLINK "http://..."` into it.
//! 3. **After `\uN`, exactly `\ucN` *characters* are skipped.** Characters, not
//!    bytes: `\'hh` is one, any control word is one, `\bin` plus its payload is
//!    one, and a brace ends the skip early.
//!
//! Nesting is bounded by [`rclip_core::MAX_DEPTH`] with a fixed-size stack and
//! a loop. There is no recursion here at all, so `{{{{{...` cannot overflow the
//! stack — it returns [`ErrorKind::DepthLimit`].

use rclip_core::{Error, ErrorKind, Result, MAX_DEPTH};

use crate::codepage::Codepage;
use crate::control;
use crate::style::CharProps;
use crate::token::{ControlSymbol, Token, Tokenizer};

/// Depth 0 is "outside any brace", so the stack needs one slot more than the
/// number of groups we allow to be open.
const STACK: usize = MAX_DEPTH as usize + 1;

/// One piece of body text together with the formatting in effect for it.
///
/// A run is *not* maximal. Consecutive runs can share identical `props` — a
/// `\uN` escape, a stream newline, or a group boundary all cut a run in half —
/// because merging them would need somewhere to put the joined text and this
/// API does not allocate. [`crate::Document`] (feature `alloc`) does the merge.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct StyledRun<'a> {
    pub text: RunText<'a>,
    pub props: CharProps,
    /// Byte offset in the input where this run starts.
    pub offset: usize,
}

/// The content of a [`StyledRun`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RunText<'a> {
    /// Literal text, borrowed straight from the input. Always ASCII.
    Text(&'a str),
    /// One character that had to be decoded: `\uN`, `\'hh`, `\~`, `\tab`, an
    /// unescaped high byte, or a named symbol like `\endash`.
    Char(char),
    /// `\par`, `\sect`, `\page`, or a backslash-newline.
    ParagraphBreak,
    /// `\line` — a soft break inside a paragraph.
    LineBreak,
}

impl RunText<'_> {
    /// `true` if this run carries no visible characters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Text(s) if s.is_empty())
    }
}

/// Where the parser currently is. Inherited by nested groups, restored on `}`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Dest {
    /// Document body: text here is output.
    Body,
    /// A destination we recognise whose content is not body text —
    /// `\fonttbl`, `\colortbl`, `\pict`, `\info`, `\header`, `\pntext`. Read it
    /// with the dedicated scanners; here it is dropped.
    Known,
    /// `{\*\unknown ...}`: dropped along with everything nested inside it.
    Ignored,
}

#[derive(Debug, Copy, Clone)]
struct Group {
    props: CharProps,
    /// The `\ucN` skip count. Per group, restored on `}`. Default 1.
    uc: u16,
    dest: Dest,
}

/// How a `\uN` parameter decoded.
enum Escape {
    /// A UTF-16 code unit, possibly half of a surrogate pair.
    Unit(u16),
    /// A scalar value written out of range of the spec's signed 16 bits.
    Scalar(char),
    /// Not representable at all.
    Invalid,
}

/// Decode a `\uN` parameter.
///
/// The spec says the value is signed 16-bit and that anything above 32767 is
/// written negative, so `\u-4145` means code unit 61391. Writers that are not
/// Word do get this wrong in both directions: some emit unsigned 0..65535, and
/// some emit the full scalar value (`\u128512` for an emoji). Both are accepted
/// here, because a reader that rejects them loses real characters and gains
/// nothing.
fn unicode_escape(p: i32) -> Escape {
    match p {
        -32768..=-1 => Escape::Unit((p + 0x1_0000) as u16),
        0..=0xFFFF => Escape::Unit(p as u16),
        0x1_0000..=0x10_FFFF => match char::from_u32(p as u32) {
            Some(c) => Escape::Scalar(c),
            None => Escape::Invalid,
        },
        _ => Escape::Invalid,
    }
}

/// Decode a `\uN` parameter to a single `char`, with no surrogate pairing.
///
/// Used where pairing is not worth a second state machine (font names); the
/// body-text parser pairs surrogates properly.
pub(crate) fn unicode_escape_char(p: i32) -> Option<char> {
    match unicode_escape(p) {
        Escape::Unit(u) => char::from_u32(u32::from(u)),
        Escape::Scalar(c) => Some(c),
        Escape::Invalid => None,
    }
}

const fn is_high_surrogate(u: u16) -> bool {
    u >= 0xD800 && u <= 0xDBFF
}

const fn is_low_surrogate(u: u16) -> bool {
    u >= 0xDC00 && u <= 0xDFFF
}

fn combine_surrogates(hi: u16, lo: u16) -> char {
    let cp = 0x1_0000 + ((u32::from(hi) - 0xD800) << 10) + (u32::from(lo) - 0xDC00);
    char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// Split `s` after `n` characters. Returns how many it actually skipped and the
/// remainder, which is a borrowed sub-slice so the fast path stays zero-copy.
fn split_skip(s: &str, n: u16) -> (u16, &str) {
    let mut taken: u16 = 0;
    for (i, _) in s.char_indices() {
        if taken == n {
            return (taken, &s[i..]);
        }
        taken += 1;
    }
    (taken, "")
}

/// Pull parser over the body text of an RTF document.
///
/// Yields one [`StyledRun`] at a time and never allocates. An `Err` is
/// terminal.
#[derive(Debug, Clone)]
pub struct Parser<'a> {
    tok: Tokenizer<'a>,
    stack: [Group; STACK],
    /// Number of open groups. `stack[depth]` is the current state.
    depth: usize,
    /// Characters still owed to the last `\uN`'s skip count.
    skip: u16,
    /// A `\*` has been seen and the next control word decides what it marks.
    pending_ignorable: bool,
    /// No token has been seen yet in the current group.
    ///
    /// `\*` only marks a destination when it is the first thing inside `{`,
    /// which is the only place any writer puts it. Honouring a stray `\*` in
    /// the middle of a group would silently drop the rest of that group -- and
    /// since a literal asterisk in body text needs no escape, a `\*` anywhere
    /// else is malformed input, not a destination marker.
    at_group_start: bool,
    /// A `\uN` high surrogate waiting for its partner.
    pending_high: Option<u16>,
    /// One run held back so a single `next()` can produce two.
    queued: Option<StyledRun<'a>>,
    codepage: Codepage,
    default_font: Option<u16>,
    done: bool,
}

impl<'a> Parser<'a> {
    /// Start parsing, verifying the `{\rtf` signature.
    ///
    /// The signature check is not pedantry: clipboard and drag-and-drop
    /// payloads are labelled by the *source*, and a flavor that claims
    /// `public.rtf` while holding HTML is a thing that happens. Failing here
    /// beats emitting the markup as text.
    pub fn new(input: &'a [u8]) -> Result<Self> {
        check_signature(input)?;
        Ok(Self::unchecked(input))
    }

    /// Start parsing without the signature check, for a fragment.
    #[must_use]
    pub fn unchecked(input: &'a [u8]) -> Self {
        const INIT: Group = Group {
            props: CharProps::DEFAULT,
            uc: 1,
            dest: Dest::Body,
        };
        Self {
            tok: Tokenizer::new(input),
            stack: [INIT; STACK],
            depth: 0,
            skip: 0,
            pending_ignorable: false,
            at_group_start: false,
            pending_high: None,
            queued: None,
            codepage: Codepage::default(),
            default_font: None,
            done: false,
        }
    }

    /// The code page in effect, which `\ansicpgN` may have changed mid-stream.
    #[must_use]
    pub const fn codepage(&self) -> Codepage {
        self.codepage
    }

    /// `\deffN`, the font index [`CharProps::font`] means by `None`.
    #[must_use]
    pub const fn default_font(&self) -> Option<u16> {
        self.default_font
    }

    fn cur(&self) -> &Group {
        // `depth` is bounded by the DepthLimit check in `group_start`, so this
        // index is never input-derived.
        &self.stack[self.depth]
    }

    fn cur_mut(&mut self) -> &mut Group {
        &mut self.stack[self.depth]
    }

    fn fail(&mut self, kind: ErrorKind, at: usize) -> Option<Result<StyledRun<'a>>> {
        self.done = true;
        Some(Err(Error::new(kind, at)))
    }

    /// Build a run, flushing an orphaned high surrogate ahead of it.
    ///
    /// Every emission path goes through here so the flush cannot be forgotten
    /// in one of them — which is exactly how lone surrogates escape into
    /// output in other implementations.
    fn emit(&mut self, text: RunText<'a>, offset: usize) -> StyledRun<'a> {
        let run = StyledRun {
            text,
            props: self.cur().props,
            offset,
        };
        if self.pending_high.take().is_some() {
            self.queued = Some(run);
            return StyledRun {
                text: RunText::Char(char::REPLACEMENT_CHARACTER),
                props: run.props,
                offset,
            };
        }
        run
    }

    /// Build a run without touching `pending_high`, for the surrogate paths
    /// that manage it themselves.
    fn raw_run(&self, text: RunText<'a>, offset: usize) -> StyledRun<'a> {
        StyledRun {
            text,
            props: self.cur().props,
            offset,
        }
    }

    fn in_body(&self) -> bool {
        self.cur().dest == Dest::Body
    }

    /// Consume `token` against the outstanding `\ucN` skip count.
    ///
    /// Returns the token (possibly shortened) if it survived, `None` if the
    /// skip swallowed it whole.
    fn apply_skip(&mut self, token: Token<'a>, offset: usize) -> Option<(Token<'a>, usize)> {
        match token {
            // Spec: "If an RTF scope delimiter character is encountered while
            // scanning skippable data, the skippable data is considered to be
            // ended before the delimiter." Without this, a `\uN` at the end of
            // a group eats the `}` and the group stack unwinds one level short
            // for the rest of the document.
            Token::GroupStart | Token::GroupEnd => {
                self.skip = 0;
                Some((token, offset))
            }
            // Spec: "a \bin keyword, its argument, and the binary data that
            // follows are considered one character". The `\bin` control word
            // already paid for it on the previous token.
            Token::Binary(_) => None,
            Token::Text(s) => {
                let (taken, rest) = split_skip(s, self.skip);
                self.skip -= taken;
                if rest.is_empty() {
                    None
                } else {
                    Some((Token::Text(rest), offset + (s.len() - rest.len())))
                }
            }
            // "Any RTF control word or symbol is considered a single character
            // for the purposes of counting skippable characters" — including
            // another `\uN`, and including `\'hh`, which is one character and
            // not two bytes.
            _ => {
                self.skip -= 1;
                None
            }
        }
    }

    /// Resolve a pending `\*` now that we can see what it marked.
    fn resolve_ignorable(&mut self, token: &Token<'a>) {
        if !self.pending_ignorable {
            return;
        }
        self.pending_ignorable = false;
        let known = match token {
            Token::ControlWord { name, .. } => control::is_known_destination(name),
            // `\*` followed by anything but a control word is malformed. Treat
            // it as ignorable: dropping a group we cannot identify is the safe
            // direction.
            _ => false,
        };
        self.cur_mut().dest = if known { Dest::Known } else { Dest::Ignored };
    }

    fn group_start(&mut self, offset: usize) -> Option<Result<StyledRun<'a>>> {
        if self.depth >= MAX_DEPTH as usize {
            return self.fail(ErrorKind::DepthLimit, offset);
        }
        // Save by copy: properties, `\ucN` and destination all inherit.
        self.stack[self.depth + 1] = self.stack[self.depth];
        self.depth += 1;
        self.at_group_start = true;
        None
    }

    fn group_end(&mut self, offset: usize) -> Option<Result<StyledRun<'a>>> {
        if self.depth == 0 {
            // A `}` with no `{`. The document is structurally broken; carrying
            // on would mean guessing which group the rest of the text is in.
            return self.fail(ErrorKind::Malformed, offset);
        }
        self.depth -= 1;
        None
    }

    /// Handle a `\uN` escape. Returns a run to emit, if any.
    fn unicode(&mut self, p: i32, offset: usize) -> Option<StyledRun<'a>> {
        // The skip count is armed even when the escape itself produces nothing
        // (a high surrogate), because the fallback characters follow either way.
        self.skip = self.cur().uc;
        if !self.in_body() {
            self.pending_high = None;
            return None;
        }
        match unicode_escape(p) {
            Escape::Unit(u) if is_high_surrogate(u) => {
                let orphan = self.pending_high.replace(u);
                orphan.map(|_| self.raw_run(RunText::Char(char::REPLACEMENT_CHARACTER), offset))
            }
            Escape::Unit(u) if is_low_surrogate(u) => {
                let c = match self.pending_high.take() {
                    Some(hi) => combine_surrogates(hi, u),
                    // An unpaired low surrogate. Word truncating text mid-pair
                    // produces these; refusing the document would be worse.
                    None => char::REPLACEMENT_CHARACTER,
                };
                Some(self.raw_run(RunText::Char(c), offset))
            }
            Escape::Unit(u) => {
                let c = char::from_u32(u32::from(u)).unwrap_or(char::REPLACEMENT_CHARACTER);
                Some(self.emit(RunText::Char(c), offset))
            }
            Escape::Scalar(c) => Some(self.emit(RunText::Char(c), offset)),
            Escape::Invalid => Some(self.emit(RunText::Char(char::REPLACEMENT_CHARACTER), offset)),
        }
    }

    /// Apply a control word. Returns a run to emit, if any.
    fn control_word(
        &mut self,
        name: &str,
        param: Option<i32>,
        offset: usize,
    ) -> Option<StyledRun<'a>> {
        // A control word with no parameter means "on"; `\word0` means "off".
        let on = param != Some(0);

        // Destination words switch the current group away from body text even
        // without a `\*`: `{\fonttbl ...}` is not document content.
        if control::is_known_destination(name) {
            self.cur_mut().dest = Dest::Known;
            return None;
        }

        match name {
            "b" => self.cur_mut().props.bold = on,
            "i" => self.cur_mut().props.italic = on,
            "strike" | "striked" => self.cur_mut().props.strike = on,
            "ulnone" => self.cur_mut().props.underline = false,
            // Every other `\ul*` is some flavour of underline. Phase 0 keeps
            // only the boolean; see CharProps::underline.
            _ if name.starts_with("ul") => self.cur_mut().props.underline = on,
            "plain" => self.cur_mut().props = CharProps::DEFAULT,
            "fs" => {
                if let Some(p) = param {
                    self.cur_mut().props.size_half_points = p.clamp(1, 0xFFFF) as u16;
                }
            }
            "f" => self.cur_mut().props.font = param.map(|p| p.clamp(0, 0xFFFF) as u16),
            "cf" => self.cur_mut().props.foreground = param.map(|p| p.clamp(0, 0xFFFF) as u16),
            // `\cb` is the spec keyword; Word writes `\highlight` instead.
            "cb" | "chcbpat" | "highlight" => {
                self.cur_mut().props.background = param.map(|p| p.clamp(0, 0xFFFF) as u16);
            }

            // Paragraph properties are not modelled in phase 0, so `\pard` has
            // nothing to reset. TODO(phase-1): alignment, indents, spacing.
            "pard" => {}
            "par" | "sect" | "page" => {
                return self
                    .in_body()
                    .then(|| self.emit(RunText::ParagraphBreak, offset))
            }
            // Tables are not modelled, but emitting *nothing* at a cell
            // boundary runs the cells together ("a1b1"), which is worse than an
            // approximation. A tab between cells and a break between rows is
            // what every RTF-to-text converter does.
            // TODO(phase-1): real table structure.
            "cell" | "nestcell" => {
                return self
                    .in_body()
                    .then(|| self.emit(RunText::Char('\t'), offset))
            }
            "row" | "nestrow" => {
                return self
                    .in_body()
                    .then(|| self.emit(RunText::ParagraphBreak, offset))
            }
            "line" | "softline" => {
                return self
                    .in_body()
                    .then(|| self.emit(RunText::LineBreak, offset))
            }
            "tab" => {
                return self
                    .in_body()
                    .then(|| self.emit(RunText::Char('\t'), offset))
            }

            "uc" => {
                // Negative is meaningless; clamp rather than reject so one bad
                // header keyword does not cost the document.
                self.cur_mut().uc = param.unwrap_or(1).clamp(0, 0xFFFF) as u16;
            }
            "u" => return self.unicode(param.unwrap_or(0), offset),

            "ansi" => self.codepage = Codepage::Windows1252,
            "ansicpg" => {
                if let Some(p) = param {
                    self.codepage = Codepage::from_ansicpg(p.clamp(0, 0xFFFF) as u16);
                }
            }
            "mac" => self.codepage = Codepage::Unsupported(10000),
            "pc" => self.codepage = Codepage::Unsupported(437),
            "pca" => self.codepage = Codepage::Unsupported(850),
            "deff" => self.default_font = param.map(|p| p.clamp(0, 0xFFFF) as u16),

            _ => {
                if let Some(c) = control::symbol_char(name) {
                    return self.in_body().then(|| self.emit(RunText::Char(c), offset));
                }
                // An unknown control word that is not `\*`-marked: ignore the
                // word, keep the group's text. This is the other half of rule 2
                // and the reason `{\fldrslt visible text}` survives.
            }
        }
        None
    }
}

impl<'a> Iterator for Parser<'a> {
    type Item = Result<StyledRun<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        // Queued first: `done` may already be set by the path that queued it.
        if let Some(q) = self.queued.take() {
            return Some(Ok(q));
        }
        if self.done {
            return None;
        }

        loop {
            let token =
                match self.tok.next() {
                    Some(Ok(t)) => t,
                    Some(Err(e)) => {
                        self.done = true;
                        return Some(Err(e));
                    }
                    None => {
                        self.done = true;
                        if self.depth > 0 {
                            // Unclosed `{`: the stream ended inside a group.
                            return Some(Err(Error::new(ErrorKind::UnexpectedEof, self.tok.pos())));
                        }
                        return self.pending_high.take().map(|_| {
                            Ok(self.raw_run(
                                RunText::Char(char::REPLACEMENT_CHARACTER),
                                self.tok.pos(),
                            ))
                        });
                    }
                };
            let mut offset = self.tok.token_offset();

            let token = if self.skip > 0 {
                match self.apply_skip(token, offset) {
                    Some((t, off)) => {
                        offset = off;
                        t
                    }
                    None => continue,
                }
            } else {
                token
            };

            // `\*` is the one token that must not resolve itself.
            if let Token::ControlSymbol(ControlSymbol::Ignorable) = token {
                self.pending_ignorable = self.at_group_start;
                continue;
            }
            self.resolve_ignorable(&token);
            self.at_group_start = false;

            // Inside an ignorable destination nothing matters but the braces.
            if self.cur().dest == Dest::Ignored
                && !matches!(token, Token::GroupStart | Token::GroupEnd)
            {
                continue;
            }

            let run = match token {
                Token::GroupStart => {
                    if let Some(e) = self.group_start(offset) {
                        return Some(e);
                    }
                    continue;
                }
                Token::GroupEnd => {
                    if let Some(e) = self.group_end(offset) {
                        return Some(e);
                    }
                    continue;
                }
                Token::ControlWord { name, param } => {
                    match self.control_word(name, param, offset) {
                        Some(run) => run,
                        None => continue,
                    }
                }
                Token::Binary(_) => continue,
                Token::Text(s) => {
                    if !self.in_body() || s.is_empty() {
                        continue;
                    }
                    self.emit(RunText::Text(s), offset)
                }
                Token::RawByte(b) => {
                    if !self.in_body() {
                        continue;
                    }
                    let c = self.codepage.decode_lossy(b);
                    self.emit(RunText::Char(c), offset)
                }
                Token::ControlSymbol(sym) => {
                    let c = match sym {
                        ControlSymbol::Literal(c) => c,
                        ControlSymbol::HexByte(b) => self.codepage.decode_lossy(b),
                        ControlSymbol::NonBreakingSpace => '\u{00A0}',
                        ControlSymbol::OptionalHyphen => '\u{00AD}',
                        ControlSymbol::NonBreakingHyphen => '\u{2011}',
                        ControlSymbol::EmbeddedParagraph => {
                            if !self.in_body() {
                                continue;
                            }
                            let run = self.emit(RunText::ParagraphBreak, offset);
                            return Some(Ok(run));
                        }
                        // `\|`, `\:` and friends carry no text.
                        ControlSymbol::Other(_) | ControlSymbol::Ignorable => continue,
                    };
                    if !self.in_body() {
                        continue;
                    }
                    self.emit(RunText::Char(c), offset)
                }
            };
            return Some(Ok(run));
        }
    }
}

/// What the document header declared.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Header {
    /// The `N` of `\rtfN`. Every writer emits 1.
    pub version: i32,
    /// `true` if the signature was `\urtf` rather than `\rtf`.
    pub unicode_variant: bool,
    pub codepage: Codepage,
    /// `\deffN`
    pub default_font: Option<u16>,
}

/// Verify `{\rtf` / `{\urtf`, allowing leading whitespace.
fn check_signature(input: &[u8]) -> Result<usize> {
    let mut i = 0;
    while matches!(input.get(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        i += 1;
    }
    if input.get(i) != Some(&b'{') {
        return Err(Error::new(ErrorKind::BadMagic, i));
    }
    i += 1;
    while matches!(input.get(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        i += 1;
    }
    let rest = input.get(i..).unwrap_or_default();
    if rest.starts_with(b"\\rtf") || rest.starts_with(b"\\urtf") {
        Ok(i)
    } else {
        Err(Error::new(ErrorKind::BadMagic, i))
    }
}

/// `true` if the bytes start with an RTF signature.
#[must_use]
pub fn is_rtf(input: &[u8]) -> bool {
    check_signature(input).is_ok()
}

/// Read the header keywords: `\rtfN`, the charset, `\ansicpgN`, `\deffN`.
///
/// Stops at the first group or text, which is where the header ends.
pub fn header(input: &[u8]) -> Result<Header> {
    check_signature(input)?;
    let mut out = Header {
        version: 1,
        unicode_variant: false,
        codepage: Codepage::default(),
        default_font: None,
    };
    let mut tok = Tokenizer::new(input);
    // The opening `{`.
    match tok.next() {
        Some(Ok(Token::GroupStart)) => {}
        Some(Err(e)) => return Err(e),
        _ => return Err(Error::new(ErrorKind::BadMagic, 0)),
    }
    for t in tok {
        match t? {
            Token::ControlWord { name, param } => match name {
                "rtf" => out.version = param.unwrap_or(1),
                "urtf" => {
                    out.unicode_variant = true;
                    out.version = param.unwrap_or(1);
                }
                "ansi" => out.codepage = Codepage::Windows1252,
                "mac" => out.codepage = Codepage::Unsupported(10000),
                "pc" => out.codepage = Codepage::Unsupported(437),
                "pca" => out.codepage = Codepage::Unsupported(850),
                "ansicpg" => {
                    if let Some(p) = param {
                        out.codepage = Codepage::from_ansicpg(p.clamp(0, 0xFFFF) as u16);
                    }
                }
                "deff" => out.default_font = param.map(|p| p.clamp(0, 0xFFFF) as u16),
                _ => {}
            },
            // The header runs until the first group (`{\fonttbl`) or body text.
            Token::GroupStart | Token::GroupEnd | Token::Text(_) => break,
            _ => {}
        }
    }
    Ok(out)
}

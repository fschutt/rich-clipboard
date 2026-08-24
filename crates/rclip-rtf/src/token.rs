//! The RTF lexer: bytes in, [`Token`]s out, no state beyond a cursor.
//!
//! Everything above this module (destinations, character properties, the
//! `\ucN` skip counter) is a state machine driven by these tokens. Keeping the
//! split sharp is deliberate: the lexer is the part that has to be *exactly*
//! right, and it is far easier to prove exactly right when it holds no
//! semantic state.
//!
//! Two lexer-level rules exist only because of the semantics above:
//!
//! - `\binN` is recognised here, not in the parser. Its payload is arbitrary
//!   bytes that routinely contain `{` and `}`; a lexer that did not consume it
//!   would hand the parser braces that are not braces and desynchronise the
//!   whole group stack. See [`Token::Binary`].
//! - CR, LF and NUL are dropped here. RTF writers wrap lines wherever they
//!   like, so a newline can land between `\uN` and its fallback character; if
//!   it survived to the parser it would be counted against the `\ucN` skip
//!   count and eat a real character. NUL is dropped because `CF_RTF` arrives
//!   off the Windows clipboard NUL-terminated.

use rclip_core::{Error, ErrorKind, Reader, Result};

/// One lexical item.
///
/// `Text` is always a borrowed slice of the input and always pure ASCII: RTF is
/// defined as a 7-bit format, so any byte >= 0x80 in the stream is a code-page
/// byte, indistinguishable in meaning from `\'hh`, and is reported as
/// [`Token::RawByte`] rather than smuggled into a `&str`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Token<'a> {
    /// `{`
    GroupStart,
    /// `}`
    GroupEnd,
    /// `\word`, `\word42`, `\word-42`, with the single delimiting space (if
    /// any) already consumed.
    ///
    /// `name` is the raw letter sequence. It is not length-limited: the spec
    /// caps control words at 32 letters, but a longer one is by definition not
    /// a spec control word, so it simply fails every lookup. Rejecting the
    /// document over it would throw away content for no safety gain.
    ControlWord { name: &'a str, param: Option<i32> },
    /// `\` followed by one non-letter.
    ControlSymbol(ControlSymbol),
    /// Literal text, guaranteed ASCII and free of `\`, `{`, `}`.
    Text(&'a str),
    /// An unescaped byte >= 0x80. Means the same thing as `\'hh`.
    RawByte(u8),
    /// The payload of a preceding `\binN`, already length-checked.
    ///
    /// Emitted as its own token so callers see the `\bin` control word first
    /// and can apply the spec rule that `\bin` plus its argument plus its data
    /// count as *one* character for `\ucN` skipping.
    Binary(&'a [u8]),
}

/// `\` followed by a single non-alphabetic character.
///
/// Control symbols take no delimiter, so a space after one is literal text.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ControlSymbol {
    /// `\\`, `\{`, `\}` — the character itself, as document text.
    Literal(char),
    /// `\*` — "ignore this destination if you do not know it".
    Ignorable,
    /// `\'hh` — one byte in the current code page.
    HexByte(u8),
    /// `\~` — non-breaking space.
    NonBreakingSpace,
    /// `\-` — optional (soft) hyphen.
    OptionalHyphen,
    /// `\_` — non-breaking hyphen.
    NonBreakingHyphen,
    /// A backslash immediately followed by CR or LF. The spec says to treat it
    /// as `\par`, and writers that hard-wrap really do rely on that.
    EmbeddedParagraph,
    /// Any other single-character symbol (`\|`, `\:`, `\<digit>`, ...).
    Other(u8),
}

/// Pull lexer over an RTF byte stream.
///
/// Yields `None` at end of input. An `Err` is terminal: the iterator fuses
/// afterwards rather than resynchronising, because every failure mode here
/// (truncated `\'h`, `\` at EOF, `\bin` past the end) means the byte stream
/// stopped being RTF, and guessing would produce confident nonsense.
#[derive(Debug, Clone)]
pub struct Tokenizer<'a> {
    r: Reader<'a>,
    token_start: usize,
    /// Bytes of `\bin` payload still owed to the caller.
    pending_bin: usize,
    done: bool,
}

impl<'a> Tokenizer<'a> {
    #[must_use]
    pub const fn new(input: &'a [u8]) -> Self {
        Self {
            r: Reader::new(input),
            token_start: 0,
            pending_bin: 0,
            done: false,
        }
    }

    /// Byte offset of the token most recently returned by [`Iterator::next`].
    ///
    /// The parser attaches this to every run it emits, so a mismatch against a
    /// corpus fixture points at a byte rather than at "somewhere in the file".
    #[must_use]
    pub const fn token_offset(&self) -> usize {
        self.token_start
    }

    /// Byte offset just past the token most recently returned.
    #[must_use]
    pub const fn pos(&self) -> usize {
        self.r.pos()
    }

    fn fail(&mut self, kind: ErrorKind, at: usize) -> Option<Result<Token<'a>>> {
        self.done = true;
        Some(Err(Error::new(kind, at)))
    }

    /// `\` has been consumed; decide what follows.
    fn escape(&mut self) -> Option<Result<Token<'a>>> {
        let rest = self.r.remaining();
        let Some(&c) = rest.first() else {
            // A trailing backslash: the stream was cut mid-escape.
            return self.fail(ErrorKind::UnexpectedEof, self.r.pos());
        };

        if c.is_ascii_alphabetic() {
            return Some(self.control_word(rest));
        }

        let at = self.r.pos();
        // Control symbols are exactly one byte wide; `\'hh` reads two more.
        if self.r.skip(1).is_err() {
            return self.fail(ErrorKind::UnexpectedEof, at);
        }
        let sym = match c {
            b'\\' => ControlSymbol::Literal('\\'),
            b'{' => ControlSymbol::Literal('{'),
            b'}' => ControlSymbol::Literal('}'),
            b'*' => ControlSymbol::Ignorable,
            b'~' => ControlSymbol::NonBreakingSpace,
            b'-' => ControlSymbol::OptionalHyphen,
            b'_' => ControlSymbol::NonBreakingHyphen,
            b'\r' | b'\n' => ControlSymbol::EmbeddedParagraph,
            b'\'' => {
                let digits = self.r.remaining();
                let (Some(hi), Some(lo)) = (digits.first(), digits.get(1)) else {
                    return self.fail(ErrorKind::UnexpectedEof, at);
                };
                let (Some(hi), Some(lo)) = (hex_val(*hi), hex_val(*lo)) else {
                    // `\'` with non-hex behind it is not recoverable: we cannot
                    // tell how many bytes the writer meant to encode.
                    return self.fail(ErrorKind::Malformed, at);
                };
                if self.r.skip(2).is_err() {
                    return self.fail(ErrorKind::UnexpectedEof, at);
                }
                ControlSymbol::HexByte(hi << 4 | lo)
            }
            other => ControlSymbol::Other(other),
        };
        Some(Ok(Token::ControlSymbol(sym)))
    }

    /// `rest` starts at the first letter of the control word.
    fn control_word(&mut self, rest: &'a [u8]) -> Result<Token<'a>> {
        let name_len = rest.iter().take_while(|b| b.is_ascii_alphabetic()).count();
        let name_bytes = &rest[..name_len];
        let name = core::str::from_utf8(name_bytes)
            .map_err(|_| Error::new(ErrorKind::InvalidUtf8, self.r.pos()))?;
        self.r.skip(name_len)?;

        let param = self.numeric_param();

        // Exactly one space, and only a space, is eaten as the delimiter. A tab
        // or a second space is document text.
        if self.r.remaining().first() == Some(&b' ') {
            self.r.skip(1)?;
        }

        // `\binN` owns the next N bytes whatever they look like.
        if name == "bin" {
            if let Some(n) = param.filter(|n| *n > 0) {
                // `n` came off the wire, so it is length-checked against the
                // buffer before it is allowed to be a slice index.
                let n = n as usize;
                self.r.check_count(n, 1)?;
                self.pending_bin = n;
            }
        }

        Ok(Token::ControlWord { name, param })
    }

    /// Optional `-` followed by at least one digit.
    ///
    /// A lone `-` is left in the stream: it is more likely to be document text
    /// than a writer emitting a parameter it forgot the digits of.
    fn numeric_param(&mut self) -> Option<i32> {
        let rest = self.r.remaining();
        let (neg, digits_at) = match rest.first() {
            Some(b'-') => (true, 1),
            _ => (false, 0),
        };
        let digits = rest.get(digits_at..)?;
        let len = digits.iter().take_while(|b| b.is_ascii_digit()).count();
        if len == 0 {
            return None;
        }
        // Saturating rather than failing: `\fs99999999999` is junk, but it is
        // junk in one property of one run, not a reason to drop the document.
        let mut acc: i64 = 0;
        for &d in &digits[..len] {
            acc = (acc * 10 + i64::from(d - b'0')).min(i64::from(i32::MAX) + 1);
        }
        let v = if neg { -acc } else { acc };
        let v = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        self.r.skip(digits_at + len).ok()?;
        Some(v)
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Result<Token<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        if self.pending_bin > 0 {
            self.token_start = self.r.pos();
            let n = core::mem::take(&mut self.pending_bin);
            return match self.r.take(n) {
                Ok(b) => Some(Ok(Token::Binary(b))),
                Err(e) => {
                    self.done = true;
                    Some(Err(e))
                }
            };
        }

        loop {
            self.token_start = self.r.pos();
            let rest = self.r.remaining();
            let Some(&b) = rest.first() else {
                self.done = true;
                return None;
            };

            match b {
                b'{' => {
                    if self.r.skip(1).is_err() {
                        return self.fail(ErrorKind::UnexpectedEof, self.token_start);
                    }
                    return Some(Ok(Token::GroupStart));
                }
                b'}' => {
                    if self.r.skip(1).is_err() {
                        return self.fail(ErrorKind::UnexpectedEof, self.token_start);
                    }
                    return Some(Ok(Token::GroupEnd));
                }
                b'\\' => {
                    if self.r.skip(1).is_err() {
                        return self.fail(ErrorKind::UnexpectedEof, self.token_start);
                    }
                    return self.escape();
                }
                // Line-wrapping artefacts and the clipboard's NUL terminator:
                // not content, and must not reach the `\ucN` skip counter.
                b'\r' | b'\n' | 0 => {
                    if self.r.skip(1).is_err() {
                        return self.fail(ErrorKind::UnexpectedEof, self.token_start);
                    }
                    continue;
                }
                0x80..=0xFF => {
                    if self.r.skip(1).is_err() {
                        return self.fail(ErrorKind::UnexpectedEof, self.token_start);
                    }
                    return Some(Ok(Token::RawByte(b)));
                }
                _ => {
                    let len = rest.iter().take_while(|&&c| is_text_byte(c)).count();
                    let bytes = &rest[..len];
                    if self.r.skip(len).is_err() {
                        return self.fail(ErrorKind::UnexpectedEof, self.token_start);
                    }
                    // Always succeeds — `is_text_byte` admits ASCII only — but
                    // asserting it in the type system beats asserting it in a
                    // comment, and costs one branch per run.
                    return match core::str::from_utf8(bytes) {
                        Ok(s) => Some(Ok(Token::Text(s))),
                        Err(_) => self.fail(ErrorKind::InvalidUtf8, self.token_start),
                    };
                }
            }
        }
    }
}

/// Bytes that may appear inside a [`Token::Text`] run.
const fn is_text_byte(b: u8) -> bool {
    b < 0x80 && b != b'\\' && b != b'{' && b != b'}' && b != b'\r' && b != b'\n' && b != 0
}

const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

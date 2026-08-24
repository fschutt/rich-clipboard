//! `Exec=` — parsed into structure, never executed.
//!
//! A `.desktop` file that arrives on the clipboard was written by another
//! process. This module turns `Exec=` into a description of a command line and
//! stops there: it does not resolve `$PATH`, does not expand field codes into
//! real filenames, and does not spawn anything. `plan/CONVENTIONS.md` rule 6
//! calls this out specifically for `.desktop`.
//!
//! # Two escape layers, in order
//!
//! §7 is explicit that "the general escape rule for values of type string ...
//! is applied before the quoting rule". So the byte stream goes through two
//! decoders:
//!
//! 1. **Value layer** (§4): `\s \n \t \r \\` become space, newline, tab,
//!    carriage return, backslash.
//! 2. **Quoting layer** (§7): `"` delimits an argument; inside one, a backslash
//!    escapes `"`, `` ` ``, `$` and `\`.
//!
//! The consequence the spec spells out: a literal backslash inside a quoted
//! argument is four backslashes in the file, and a literal `$` is `\\$`. That
//! only works if the layers run in this order, and it is why the scanner below
//! decodes value escapes *while* tracking quotes rather than before or after.
//!
//! A worked example — the file contains
//!
//! ```text
//! Exec=/bin/prog "a\\\\b" %U
//! ```
//!
//! The value layer turns `\\\\` into `\\`; the quoting layer turns `\\` into
//! one `\`. The second argument is `a\b`.
//!
//! # Where this is deliberately lenient
//!
//! `\"` is not a valid *value* escape, so `Exec=sh -c "echo \"hi\""` is
//! strictly malformed and GLib rejects it — but people write it constantly.
//! The scanner treats any `\X` pair as one unit for the purpose of finding
//! argument and quote boundaries, so such a file still splits into the
//! arguments its author intended; [`ExecArg::pieces`] then accepts `\"`,
//! ``\` ``, `\$` as quote-layer escapes. Since nothing here is ever executed,
//! reading such a file is not a hazard, and refusing to is a lost paste.

use rclip_core::{Error, ErrorKind, Result};

/// A field code from §7.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FieldCode {
    /// `%f` — a single local file path.
    SingleFile,
    /// `%F` — a list of local file paths, one argument each.
    FileList,
    /// `%u` — a single URL.
    SingleUrl,
    /// `%U` — a list of URLs, one argument each.
    UrlList,
    /// `%i` — expands to two arguments, `--icon` and the `Icon` value, or to
    /// nothing when `Icon` is absent.
    Icon,
    /// `%c` — the translated `Name`.
    TranslatedName,
    /// `%k` — the location of the desktop file itself.
    DesktopFileLocation,
    /// `%d %D %n %N %v %m` — deprecated. §7: "Deprecated field codes should be
    /// removed from the command line and ignored."
    Deprecated(char),
}

impl FieldCode {
    /// Recognize the character after a `%`.
    ///
    /// Returns `None` for anything not in §7. That is not a "pass it through"
    /// case: §7 says "command lines that contain a field code that is not
    /// listed in this specification are invalid and must not be processed",
    /// because an implementation that invented `%x` would be a way to smuggle
    /// an argument past every other implementation.
    #[must_use]
    pub const fn from_char(c: char) -> Option<Self> {
        Some(match c {
            'f' => Self::SingleFile,
            'F' => Self::FileList,
            'u' => Self::SingleUrl,
            'U' => Self::UrlList,
            'i' => Self::Icon,
            'c' => Self::TranslatedName,
            'k' => Self::DesktopFileLocation,
            'd' | 'D' | 'n' | 'N' | 'v' | 'm' => Self::Deprecated(c),
            _ => return None,
        })
    }

    /// The character this code is spelled with.
    #[must_use]
    pub const fn as_char(self) -> char {
        match self {
            Self::SingleFile => 'f',
            Self::FileList => 'F',
            Self::SingleUrl => 'u',
            Self::UrlList => 'U',
            Self::Icon => 'i',
            Self::TranslatedName => 'c',
            Self::DesktopFileLocation => 'k',
            Self::Deprecated(c) => c,
        }
    }

    /// `true` for the codes §7 marks deprecated.
    #[must_use]
    pub const fn is_deprecated(self) -> bool {
        matches!(self, Self::Deprecated(_))
    }

    /// `true` for the codes that name the documents to open — at most one of
    /// these may appear in a command line.
    #[must_use]
    pub const fn is_document(self) -> bool {
        matches!(self, Self::SingleFile | Self::FileList | Self::SingleUrl | Self::UrlList)
    }

    /// `true` for the codes §7 says "may only be used as an argument on their
    /// own", because they can expand to more than one argument.
    #[must_use]
    pub const fn must_stand_alone(self) -> bool {
        matches!(self, Self::FileList | Self::UrlList | Self::Icon)
    }
}

/// One decoded piece of an argument.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExecPiece {
    /// A literal character, both escape layers already applied. `%%` arrives
    /// here as `'%'`.
    Char(char),
    /// A field code, to be expanded by whoever launches the program — not by
    /// this crate.
    Field(FieldCode),
}

/// A parsed `Exec=` command line.
#[derive(Debug, Copy, Clone)]
pub struct ExecCommand<'a> {
    raw: &'a str,
    offset: usize,
}

impl<'a> ExecCommand<'a> {
    /// Wrap the raw value of an `Exec=` key.
    #[must_use]
    pub const fn new(raw: &'a str, offset: usize) -> Self {
        Self { raw, offset }
    }

    /// The value as written.
    #[must_use]
    pub const fn raw(&self) -> &'a str {
        self.raw
    }

    /// Split into arguments.
    #[must_use]
    pub const fn args(&self) -> ExecArgs<'a> {
        ExecArgs { rest: self.raw, offset: self.offset }
    }

    /// The program, i.e. the first argument.
    ///
    /// # Errors
    ///
    /// Whatever [`ExecArgs`] reports for the first argument.
    pub fn program(&self) -> Option<Result<ExecArg<'a>>> {
        self.args().next()
    }

    /// Check the §7 rules that span the whole command line.
    ///
    /// These are worth a separate pass because each one is a way for a crafted
    /// file to get a launcher to pass more arguments than the author of the
    /// launcher expected:
    ///
    /// - "A command line may contain at most one `%f`, `%u`, `%F` or `%U`."
    /// - "`%F`, `%U` and `%i` may only be used as an argument on their own" —
    ///   they expand to a variable number of arguments.
    /// - "Field codes must not be used inside a quoted argument."
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] at the offending argument, or the first error
    /// from decoding an argument.
    pub fn validate(&self) -> Result<()> {
        let mut documents = 0u32;
        for arg in self.args() {
            let arg = arg?;
            let mut pieces = 0u32;
            let mut standalone: Option<FieldCode> = None;
            for piece in arg.pieces() {
                pieces += 1;
                if let ExecPiece::Field(f) = piece? {
                    if arg.quoted() {
                        return Err(Error::new(ErrorKind::Malformed, arg.offset()));
                    }
                    if f.is_document() {
                        documents += 1;
                    }
                    if f.must_stand_alone() {
                        standalone = Some(f);
                    }
                }
            }
            if standalone.is_some() && pieces != 1 {
                return Err(Error::new(ErrorKind::Malformed, arg.offset()));
            }
        }
        if documents > 1 {
            return Err(Error::new(ErrorKind::Malformed, self.offset));
        }
        Ok(())
    }
}

/// One argument of an [`ExecCommand`].
#[derive(Debug, Copy, Clone)]
pub struct ExecArg<'a> {
    raw: &'a str,
    offset: usize,
    quoted: bool,
}

impl<'a> ExecArg<'a> {
    /// The argument text as written, with the delimiting quotes removed if it
    /// had any, and both escape layers still applied.
    #[must_use]
    pub const fn raw(&self) -> &'a str {
        self.raw
    }

    /// Byte offset of the argument in the input buffer.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// `true` if the argument was written between double quotes.
    #[must_use]
    pub const fn quoted(&self) -> bool {
        self.quoted
    }

    /// Decode the argument into literal characters and field codes.
    #[must_use]
    pub const fn pieces(&self) -> ExecPieces<'a> {
        ExecPieces { rest: self.raw, offset: self.offset, quoted: self.quoted, done: false }
    }

    /// The field code, if the whole argument is exactly one.
    ///
    /// This is the shape a launcher actually branches on: `%U` on its own means
    /// "put the URLs here", whereas `--file=%u` means "substitute into this
    /// text".
    #[must_use]
    pub fn as_field(&self) -> Option<FieldCode> {
        let mut it = self.pieces();
        let first = it.next()?.ok()?;
        if it.next().is_some() {
            return None;
        }
        match first {
            ExecPiece::Field(f) => Some(f),
            ExecPiece::Char(_) => None,
        }
    }
}

/// Iterator over the arguments of an [`ExecCommand`].
#[derive(Debug, Copy, Clone)]
pub struct ExecArgs<'a> {
    rest: &'a str,
    offset: usize,
}

impl<'a> ExecArgs<'a> {
    /// Advance past `n` bytes of `rest`.
    fn bump(&mut self, n: usize) {
        self.rest = self.rest.get(n..).unwrap_or("");
        self.offset += n;
    }
}

/// `true` for the characters that end an unquoted argument.
///
/// Whitespace here is post-value-layer: `\s` decodes to a space and therefore
/// separates, which is why §7 says an argument containing a reserved character
/// "must be quoted". `\t`, `\n` and `\r` are treated the same way, matching
/// `g_shell_parse_argv`, which is what GIO feeds the unescaped value to.
const fn is_arg_separator(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// Decode one value-layer unit at the head of `s`.
///
/// Returns the decoded character, whether it came from an escape, and how many
/// raw bytes it occupied. An *invalid* escape still reports two bytes: whether
/// `\q` is an error is [`ExecPieces`]'s business, and answering it here would
/// move the argument boundaries.
fn value_unit(s: &str) -> Option<(char, bool, usize)> {
    let mut it = s.chars();
    let c = it.next()?;
    if c != '\\' {
        return Some((c, false, c.len_utf8()));
    }
    match it.next() {
        // A dangling backslash. Report it as itself so the scanner terminates;
        // `ExecPieces` turns it into an error.
        None => Some(('\\', false, 1)),
        Some(e) => {
            let decoded = match e {
                's' => ' ',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' => '\\',
                other => other,
            };
            Some((decoded, true, 1 + e.len_utf8()))
        }
    }
}

impl<'a> Iterator for ExecArgs<'a> {
    type Item = Result<ExecArg<'a>>;

    fn next(&mut self) -> Option<Result<ExecArg<'a>>> {
        // Skip separators. A separator produced by `\s` counts, so this has to
        // go through the value layer rather than `trim_start`.
        loop {
            let (c, _escaped, len) = value_unit(self.rest)?;
            // The test is on the *decoded* character: `\s` decodes to a space
            // and separates, which is exactly why §7 requires an argument
            // containing a space to be quoted.
            if is_arg_separator(c) {
                self.bump(len);
            } else {
                break;
            }
        }

        let offset = self.offset;
        let (first, first_escaped, first_len) = value_unit(self.rest)?;
        if first == '"' && !first_escaped {
            self.bump(first_len);
            let content = self.rest;
            let mut consumed = 0usize;
            let mut pending_backslash = false;
            loop {
                let Some((c, escaped, len)) = value_unit(content.get(consumed..).unwrap_or(""))
                else {
                    // Ran out of input with the quote still open. §7 has no
                    // recovery for this and guessing where the argument ended
                    // would change what a launcher is told to run.
                    return Some(Err(Error::new(ErrorKind::Malformed, offset)));
                };
                if pending_backslash {
                    pending_backslash = false;
                    consumed += len;
                    continue;
                }
                if c == '\\' && escaped {
                    // A value-layer `\\` produced a real backslash, which is
                    // the *quoting* layer's escape character; it protects the
                    // next decoded character, closing quote included.
                    pending_backslash = true;
                    consumed += len;
                    continue;
                }
                if c == '"' && !escaped {
                    let raw = content.get(..consumed).unwrap_or("");
                    self.bump(consumed + len);
                    return Some(Ok(ExecArg { raw, offset, quoted: true }));
                }
                consumed += len;
            }
        }

        let content = self.rest;
        let mut consumed = 0usize;
        while let Some((c, _escaped, len)) = value_unit(content.get(consumed..).unwrap_or("")) {
            if is_arg_separator(c) {
                break;
            }
            consumed += len;
        }
        let raw = content.get(..consumed).unwrap_or("");
        self.bump(consumed);
        Some(Ok(ExecArg { raw, offset, quoted: false }))
    }
}

/// Iterator over the decoded pieces of an [`ExecArg`].
#[derive(Debug, Copy, Clone)]
pub struct ExecPieces<'a> {
    rest: &'a str,
    offset: usize,
    quoted: bool,
    done: bool,
}

impl Iterator for ExecPieces<'_> {
    type Item = Result<ExecPiece>;

    fn next(&mut self) -> Option<Result<ExecPiece>> {
        if self.done {
            return None;
        }
        let at = self.offset;
        let mut it = self.rest.chars();
        let c = it.next()?;

        // `%` is never produced by an escape, so it is always structural.
        if c == '%' {
            let Some(code) = it.next() else {
                self.done = true;
                return Some(Err(Error::new(ErrorKind::UnexpectedEof, at)));
            };
            self.rest = it.as_str();
            self.offset += 1 + code.len_utf8();
            if code == '%' {
                return Some(Ok(ExecPiece::Char('%')));
            }
            return Some(match FieldCode::from_char(code) {
                Some(f) => Ok(ExecPiece::Field(f)),
                None => {
                    self.done = true;
                    Err(Error::new(ErrorKind::Malformed, at))
                }
            });
        }

        if c != '\\' {
            self.rest = it.as_str();
            self.offset += c.len_utf8();
            return Some(Ok(ExecPiece::Char(c)));
        }

        let Some(esc) = it.next() else {
            self.done = true;
            return Some(Err(Error::new(ErrorKind::UnexpectedEof, at)));
        };
        let step = 1 + esc.len_utf8();
        match esc {
            's' | 'n' | 't' | 'r' => {
                self.rest = it.as_str();
                self.offset += step;
                let d = match esc {
                    's' => ' ',
                    'n' => '\n',
                    't' => '\t',
                    _ => '\r',
                };
                Some(Ok(ExecPiece::Char(d)))
            }
            '\\' => {
                // Value layer produced one backslash. In a quoted argument that
                // backslash belongs to the quoting layer and escapes whatever
                // comes next; unquoted, there is no quoting layer, so it is a
                // literal backslash.
                if !self.quoted {
                    self.rest = it.as_str();
                    self.offset += step;
                    return Some(Ok(ExecPiece::Char('\\')));
                }
                let Some((next, _, next_len)) = value_unit(it.as_str()) else {
                    self.done = true;
                    return Some(Err(Error::new(ErrorKind::UnexpectedEof, at)));
                };
                self.rest = it.as_str().get(next_len..).unwrap_or("");
                self.offset += step + next_len;
                Some(Ok(ExecPiece::Char(next)))
            }
            // Not a value escape. See the module docs: in a quoted argument
            // these are what people actually write for the quoting-layer
            // escapes, and accepting them cannot make anything run.
            '"' | '`' | '$' if self.quoted => {
                self.rest = it.as_str();
                self.offset += step;
                Some(Ok(ExecPiece::Char(esc)))
            }
            _ => {
                self.done = true;
                Some(Err(Error::new(ErrorKind::Malformed, at)))
            }
        }
    }
}

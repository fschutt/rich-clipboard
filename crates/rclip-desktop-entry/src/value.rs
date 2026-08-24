//! Values, escape sequences and `;`-separated lists.
//!
//! This is the part of the Desktop Entry Spec that existing parsers get wrong,
//! so it is the part with the most comments.
//!
//! Two rules from §4 have to hold at once:
//!
//! 1. `\s \n \t \r \\` are escape sequences in `string`, `localestring` and
//!    `iconstring` values.
//! 2. A multi-valued key is separated by `;`, and "semicolons in these values
//!    need to be escaped using `\;`".
//!
//! The trap is the interaction: **split first, unescape second.** Unescaping
//! first turns `\;` into a bare `;`, and the split then treats it as a
//! separator — exactly the boundary the escape existed to suppress. Both
//! `freedesktop-file-parser` (a naive `.split(";")` with no unescaping at all)
//! and `freedesktop-desktop-entry` (unescape at parse time, split afterwards)
//! get this wrong. [`ListItems`] therefore scans for separators over the *raw*
//! text, stepping over `\X` pairs, and hands each piece back still escaped.
//!
//! The second trap is the trailing separator. §4 says the value "may be
//! optionally terminated by a semicolon" and that "trailing empty strings must
//! always be terminated with a semicolon" — so `a;b;` is two items and `a;b;;`
//! is three, the last one empty. A plain `split(';')` produces a spurious empty
//! item for the common case.

use rclip_core::{Error, ErrorKind, Result};

/// A raw, still-escaped value.
///
/// Nothing is decoded on construction. Decoding can fail (an unknown escape, a
/// dangling backslash), and a parser that decoded eagerly would have to fail
/// the whole file over one bad `Comment=`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Value<'a> {
    raw: &'a str,
    offset: usize,
}

impl<'a> Value<'a> {
    pub(crate) const fn new(raw: &'a str, offset: usize) -> Self {
        Self { raw, offset }
    }

    /// The value exactly as it appears in the file, escapes and all.
    #[must_use]
    pub const fn raw(&self) -> &'a str {
        self.raw
    }

    /// Byte offset of the first character of the value in the input buffer.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// Decode the escape sequences, one `char` at a time.
    ///
    /// Yields `Err` and then stops, rather than substituting or skipping: an
    /// invalid escape means the producer and this parser disagree about the
    /// text, and silently guessing is how a `Name=` ends up displaying somebody
    /// else's `Exec=`.
    #[must_use]
    pub const fn chars(&self) -> Unescape<'a> {
        Unescape {
            rest: self.raw,
            offset: self.offset,
            done: false,
        }
    }

    /// The items of a `string(s)` / `localestring(s)` value.
    ///
    /// Each item is itself a [`Value`], still escaped — call [`Value::chars`]
    /// on it. See the module docs for why the split happens first.
    #[must_use]
    pub const fn items(&self) -> ListItems<'a> {
        ListItems {
            rest: if self.raw.is_empty() {
                None
            } else {
                Some(self.raw)
            },
            offset: self.offset,
        }
    }

    /// Compare the decoded value against a plain string without allocating.
    ///
    /// Used for the handful of keys whose values are fixed vocabulary —
    /// `Type=Application`, `Terminal=true` — where building a `String` to
    /// compare four characters would be the only reason the crate needed
    /// `alloc`.
    #[must_use]
    pub fn eq_str(&self, s: &str) -> bool {
        let mut expected = s.chars();
        for got in self.chars() {
            match (got, expected.next()) {
                (Ok(a), Some(b)) if a == b => {}
                _ => return false,
            }
        }
        expected.next().is_none()
    }

    /// A `boolean` value.
    ///
    /// §4: "Values of type boolean must either be the string `true` or
    /// `false`." Deliberately not accepting `1`/`0`/`True` — those were the
    /// pre-1.0 spelling, are listed under Deprecated Items, and accepting them
    /// silently would mean two readers of the same file disagree.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] for anything else.
    pub fn as_bool(&self) -> Result<bool> {
        if self.eq_str("true") {
            Ok(true)
        } else if self.eq_str("false") {
            Ok(false)
        } else {
            Err(Error::new(ErrorKind::Malformed, self.offset))
        }
    }

    /// A `numeric` value — "a valid floating point number as recognized by the
    /// `%f` specifier for `scanf` in the C locale".
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] if the text does not parse as an `f64`.
    pub fn as_f64(&self) -> Result<f64> {
        // Numeric values contain no escapes, so the raw text is the value.
        self.raw
            .parse::<f64>()
            .map_err(|_| Error::new(ErrorKind::Malformed, self.offset))
    }
}

/// Iterator over the decoded characters of a [`Value`].
#[derive(Debug, Copy, Clone)]
pub struct Unescape<'a> {
    rest: &'a str,
    offset: usize,
    done: bool,
}

impl<'a> Unescape<'a> {
    /// Byte offset of the next character to be decoded.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }
}

impl Iterator for Unescape<'_> {
    type Item = Result<char>;

    fn next(&mut self) -> Option<Result<char>> {
        if self.done {
            return None;
        }
        let mut it = self.rest.chars();
        let c = it.next()?;
        if c != '\\' {
            self.rest = it.as_str();
            self.offset += c.len_utf8();
            return Some(Ok(c));
        }
        let at = self.offset;
        let Some(esc) = it.next() else {
            // A value ending in a lone backslash. The producer meant to escape
            // something and the line ended first; there is no safe guess.
            self.done = true;
            return Some(Err(Error::new(ErrorKind::UnexpectedEof, at)));
        };
        let Some(decoded) = decode_escape(esc) else {
            self.done = true;
            return Some(Err(Error::new(ErrorKind::Malformed, at)));
        };
        self.rest = it.as_str();
        self.offset += 1 + esc.len_utf8();
        Some(Ok(decoded))
    }
}

/// The escape table from §4, plus `\;`.
///
/// `\;` is only *required* inside a multi-valued key, and GLib rejects it
/// elsewhere. It is unambiguous everywhere, though, and rejecting
/// `Comment=either\;or` gains nothing and loses the comment, so it is accepted
/// in every value.
const fn decode_escape(esc: char) -> Option<char> {
    Some(match esc {
        's' => ' ',
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        '\\' => '\\',
        ';' => ';',
        _ => return None,
    })
}

/// Iterator over the items of a `;`-separated value.
#[derive(Debug, Copy, Clone)]
pub struct ListItems<'a> {
    /// `None` once the list is exhausted. Distinct from `Some("")`, which is a
    /// final empty item that the producer explicitly terminated.
    rest: Option<&'a str>,
    offset: usize,
}

impl<'a> Iterator for ListItems<'a> {
    type Item = Value<'a>;

    fn next(&mut self) -> Option<Value<'a>> {
        let rest = self.rest?;
        let offset = self.offset;
        match separator(rest) {
            Some(i) => {
                let item = rest.get(..i)?;
                let after = rest.get(i + 1..)?;
                // An empty tail means the value ended with a separator, which
                // §4 defines as an optional terminator rather than an extra
                // empty item.
                self.rest = if after.is_empty() { None } else { Some(after) };
                self.offset += i + 1;
                Some(Value::new(item, offset))
            }
            None => {
                self.rest = None;
                Some(Value::new(rest, offset))
            }
        }
    }
}

/// Byte index of the first `;` that is not part of a `\;` escape.
///
/// Walks the raw text so `\;` never separates and `\;` always does — the
/// second backslash is consumed as the escape's payload, leaving the `;` bare.
fn separator(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            // Step over the escape and whatever it escapes. Even an invalid
            // escape is stepped over: whether `\q` is an error is a question
            // for `Unescape`, and answering it here would change where the
            // list splits.
            b'\\' => i += 2,
            b';' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

#[cfg(feature = "alloc")]
mod with_alloc {
    extern crate alloc;

    use alloc::string::String;
    use rclip_core::Result;

    use super::Value;

    impl Value<'_> {
        /// Decode into an owned `String`.
        ///
        /// # Errors
        ///
        /// The first error from [`Value::chars`].
        pub fn to_unescaped(&self) -> Result<String> {
            let mut out = String::with_capacity(self.raw().len());
            for c in self.chars() {
                out.push(c?);
            }
            Ok(out)
        }

        /// Decode, passing an invalid escape through as the literal characters
        /// that spelled it.
        ///
        /// For displaying a value from a file you do not control and would
        /// rather render imperfectly than not at all. Prefer
        /// [`Value::to_unescaped`] anywhere the exact text matters.
        #[must_use]
        pub fn to_unescaped_lossy(&self) -> String {
            let mut out = String::with_capacity(self.raw().len());
            let mut rest = self.raw();
            loop {
                let mut it = rest.chars();
                let Some(c) = it.next() else { break };
                if c != '\\' {
                    out.push(c);
                    rest = it.as_str();
                    continue;
                }
                match it.next() {
                    // Dangling backslash at the end of the value: keep it.
                    None => {
                        out.push('\\');
                        break;
                    }
                    Some(esc) => {
                        match super::decode_escape(esc) {
                            Some(d) => out.push(d),
                            None => {
                                out.push('\\');
                                out.push(esc);
                            }
                        }
                        rest = it.as_str();
                    }
                }
            }
            out
        }
    }
}

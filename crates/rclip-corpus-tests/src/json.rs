//! A JSON reader for one shape: an object whose values are scalars, or flat
//! arrays of them.
//!
//! That is the whole sidecar contract (`corpus/README.md`), so this is the
//! whole parser. It refuses nested objects and nested arrays rather than
//! skipping them, which turns "somebody invented a tree-shaped sidecar" into a
//! failing test instead of a silently ignored key. A one-level array is in,
//! because pinning a list — the two paths a `CF_HDROP` fixture decodes to — is
//! a thing a sidecar legitimately wants to do.
//!
//! Escapes are decoded properly — `\uXXXX`, surrogate pairs and all — because
//! the leak scanner reads sidecar prose, and a scanner that only saw the raw
//! file would miss a name spelled `\u0066\u0072\u0065\u0064` while a human
//! reading the rendered sidecar would not.

use std::collections::BTreeMap;
use std::fmt;

/// A scalar sidecar value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A JSON string, with every escape already decoded.
    Str(String),
    /// `true` / `false`.
    Bool(bool),
    /// `null`.
    Null,
    /// A number, kept as the text that spelled it, so a byte count and a
    /// code-page number survive without a float rounding either.
    Number(String),
    /// A one-level array of scalars.
    Array(Vec<Value>),
}

impl Value {
    /// The string behind a [`Value::Str`], or `None` for every other kind.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The elements of a [`Value::Array`].
    #[must_use]
    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(v) => Some(v),
            _ => None,
        }
    }

    /// The boolean behind a [`Value::Bool`].
    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The name of this kind, with its article, for error messages.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Str(_) => "a string",
            Self::Bool(_) => "a boolean",
            Self::Null => "null",
            Self::Number(_) => "a number",
            Self::Array(_) => "an array",
        }
    }
}

/// A parse failure, located in the sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// Byte offset into the sidecar text.
    pub offset: usize,
    /// What the reader wanted to see.
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for Error {}

/// A parsed sidecar: keys in file order is not preserved, but the corpus never
/// depends on order and a `BTreeMap` makes failure messages deterministic.
pub type Object = BTreeMap<String, Value>;

/// Parse one flat JSON object.
///
/// # Errors
///
/// Any deviation from "a flat object of scalars", including a duplicate key —
/// which JSON permits and which would silently discard one of the two values.
pub fn parse_object(src: &str) -> Result<Object, Error> {
    let mut p = Parser {
        b: src.as_bytes(),
        i: 0,
    };
    p.ws();
    p.expect(b'{')?;
    let mut out = Object::new();
    p.ws();
    if p.peek() == Some(b'}') {
        p.i += 1;
        p.ws();
        return p.finish(out);
    }
    loop {
        p.ws();
        let key_at = p.i;
        let key = p.string()?;
        p.ws();
        p.expect(b':')?;
        p.ws();
        let value = p.value()?;
        if out.insert(key.clone(), value).is_some() {
            return Err(Error {
                offset: key_at,
                message: format!("duplicate key {key:?}"),
            });
        }
        p.ws();
        match p.next() {
            Some(b',') => {}
            Some(b'}') => break,
            _ => {
                return Err(Error {
                    offset: p.i.saturating_sub(1),
                    message: "expected ',' or '}'".into(),
                })
            }
        }
    }
    p.ws();
    p.finish(out)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn finish(&self, out: Object) -> Result<Object, Error> {
        if self.i == self.b.len() {
            Ok(out)
        } else {
            Err(Error {
                offset: self.i,
                message: "trailing bytes after the top-level object".into(),
            })
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn expect(&mut self, want: u8) -> Result<(), Error> {
        if self.peek() == Some(want) {
            self.i += 1;
            Ok(())
        } else {
            Err(Error {
                offset: self.i,
                message: format!("expected {:?}", want as char),
            })
        }
    }

    fn lit(&mut self, word: &str) -> Result<(), Error> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(())
        } else {
            Err(Error {
                offset: self.i,
                message: format!("expected {word}"),
            })
        }
    }

    fn value(&mut self) -> Result<Value, Error> {
        match self.peek() {
            Some(b'"') => self.string().map(Value::Str),
            Some(b't') => self.lit("true").map(|()| Value::Bool(true)),
            Some(b'f') => self.lit("false").map(|()| Value::Bool(false)),
            Some(b'n') => self.lit("null").map(|()| Value::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(b'[') => self.array(),
            // The contract, enforced rather than documented.
            Some(b'{') => Err(Error {
                offset: self.i,
                message: "a sidecar value is a scalar or a flat array of scalars, \
                          never a nested object"
                    .into(),
            }),
            _ => Err(Error {
                offset: self.i,
                message: "expected a value".into(),
            }),
        }
    }

    /// One level of array, scalars only.
    fn array(&mut self) -> Result<Value, Error> {
        let open = self.i;
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.ws();
            if matches!(self.peek(), Some(b'[' | b'{')) {
                return Err(Error {
                    offset: self.i,
                    message: "a sidecar array holds scalars, not more structure".into(),
                });
            }
            items.push(self.value()?);
            self.ws();
            match self.next() {
                Some(b',') => {}
                Some(b']') => return Ok(Value::Array(items)),
                _ => {
                    return Err(Error {
                        offset: open,
                        message: "expected ',' or ']'".into(),
                    })
                }
            }
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.i += 1;
        }
        if self.i == start {
            return Err(Error {
                offset: start,
                message: "expected a number".into(),
            });
        }
        Ok(Value::Number(
            String::from_utf8_lossy(&self.b[start..self.i]).into_owned(),
        ))
    }

    fn string(&mut self) -> Result<String, Error> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let at = self.i;
            let c = self.next().ok_or_else(|| Error {
                offset: at,
                message: "unterminated string".into(),
            })?;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let esc_at = self.i;
                    let e = self.next().ok_or_else(|| Error {
                        offset: esc_at,
                        message: "unterminated escape".into(),
                    })?;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape(esc_at)?),
                        other => {
                            return Err(Error {
                                offset: esc_at,
                                message: format!("unknown escape \\{}", other as char),
                            })
                        }
                    }
                }
                // Control characters must be escaped in JSON. Rejecting a raw
                // one catches a sidecar written by `echo` rather than by a
                // writer that knows the rules.
                0x00..=0x1f => {
                    return Err(Error {
                        offset: at,
                        message: format!("unescaped control character 0x{c:02x}"),
                    })
                }
                _ => {
                    // Copy the whole UTF-8 sequence; the input is a `&str`, so
                    // it is well-formed by construction.
                    let len = utf8_len(c);
                    let end = (at + len).min(self.b.len());
                    out.push_str(&String::from_utf8_lossy(&self.b[at..end]));
                    self.i = end;
                }
            }
        }
    }

    /// `\uXXXX`, joining a surrogate pair with the `\uXXXX` that follows it.
    fn unicode_escape(&mut self, esc_at: usize) -> Result<char, Error> {
        let hi = self.hex4(esc_at)?;
        if !(0xd800..0xdc00).contains(&hi) {
            return char::from_u32(u32::from(hi)).ok_or_else(|| Error {
                offset: esc_at,
                message: format!("\\u{hi:04x} is not a character"),
            });
        }
        // High surrogate: a low one has to follow, or the escape names half a
        // character.
        if !self.b[self.i..].starts_with(b"\\u") {
            return Err(Error {
                offset: esc_at,
                message: "high surrogate with no \\u low surrogate after it".into(),
            });
        }
        self.i += 2;
        let lo = self.hex4(esc_at)?;
        if !(0xdc00..0xe000).contains(&lo) {
            return Err(Error {
                offset: esc_at,
                message: format!("\\u{lo:04x} is not a low surrogate"),
            });
        }
        let c = 0x1_0000 + ((u32::from(hi) - 0xd800) << 10) + (u32::from(lo) - 0xdc00);
        char::from_u32(c).ok_or_else(|| Error {
            offset: esc_at,
            message: "surrogate pair does not name a character".into(),
        })
    }

    fn hex4(&mut self, esc_at: usize) -> Result<u16, Error> {
        let end = self.i + 4;
        let digits = self.b.get(self.i..end).ok_or_else(|| Error {
            offset: esc_at,
            message: "\\u needs four hex digits".into(),
        })?;
        let mut v: u16 = 0;
        for &d in digits {
            let n = (d as char).to_digit(16).ok_or_else(|| Error {
                offset: esc_at,
                message: "\\u needs four hex digits".into(),
            })?;
            v = v * 16 + n as u16;
        }
        self.i = end;
        Ok(v)
    }
}

const fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_object, Value};

    #[test]
    fn reads_the_shape_a_sidecar_actually_has() {
        let o = parse_object(
            r#"{
              "format": "CF_HDROP",
              "origin": "synthetic",
              "expect": "ok",
              "redacted": false,
              "notes": "a \"quoted\" word, a backslash \\, and ö ペ"
            }"#,
        )
        .unwrap();
        assert_eq!(o["format"], Value::Str("CF_HDROP".into()));
        assert_eq!(o["redacted"], Value::Bool(false));
        assert_eq!(
            o["notes"].as_str().unwrap(),
            "a \"quoted\" word, a backslash \\, and ö ペ"
        );
    }

    #[test]
    fn surrogate_pairs_join() {
        let o = parse_object(r#"{"n": "😊"}"#).unwrap();
        assert_eq!(o["n"].as_str().unwrap(), "\u{1f60a}");
    }

    #[test]
    fn a_lone_high_surrogate_is_an_error_not_a_replacement_char() {
        assert!(parse_object(r#"{"n": "\ud83d!"}"#).is_err());
    }

    #[test]
    fn nesting_is_refused_but_one_level_of_array_is_not() {
        let e = parse_object(r#"{"a": {"b": "c"}}"#).unwrap_err();
        assert!(e.message.contains("nested object"), "{e}");
        assert!(parse_object(r#"{"a": [["b"]]}"#).is_err());

        let o = parse_object(r#"{"a": ["x", "y"], "b": []}"#).unwrap();
        let items = o["a"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), Some("x"));
        assert!(o["b"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_duplicate_key_is_an_error_not_a_silent_overwrite() {
        assert!(parse_object(r#"{"expect": "ok", "expect": "error"}"#).is_err());
    }

    #[test]
    fn empty_object_and_trailing_junk() {
        assert!(parse_object("{}").unwrap().is_empty());
        assert!(parse_object(r#"{"a":"b"} tail"#).is_err());
        assert!(parse_object(r#"{"a":"b",}"#).is_err());
    }

    #[test]
    fn null_and_numbers_survive() {
        let o = parse_object(r#"{"a": null, "b": -1.5e3}"#).unwrap();
        assert_eq!(o["a"], Value::Null);
        assert_eq!(o["b"], Value::Number("-1.5e3".into()));
    }
}

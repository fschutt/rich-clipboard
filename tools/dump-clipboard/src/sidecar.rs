//! The `.json` sidecar every `.bin` carries, per `corpus/README.md`.
//!
//! Written by hand rather than through `serde_json` for two reasons: the key
//! order is part of the file (every sidecar in the corpus reads the same way
//! down the page, and a diff of a re-capture should show the byte count
//! changing, not the whole object reshuffling), and a capture tool that links
//! four different OS APIs has enough dependencies already.

use std::fmt::Write as _;

/// The only three JSON shapes a sidecar needs.
#[derive(Debug, Clone)]
pub enum Value {
    Str(String),
    Uint(u64),
    Bool(bool),
    Null,
}

impl Value {
    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }
}

/// Render an object, one key per line, in the order given.
pub fn object(fields: &[(&str, Value)]) -> String {
    let mut out = String::from("{\n");
    for (i, (key, value)) in fields.iter().enumerate() {
        out.push_str("  ");
        escape_into(key, &mut out);
        out.push_str(": ");
        match value {
            Value::Str(s) => escape_into(s, &mut out),
            Value::Uint(n) => {
                let _ = write!(out, "{n}");
            }
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Null => out.push_str("null"),
        }
        if i + 1 < fields.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("}\n");
    out
}

/// Quote and escape a string.
///
/// Native identifiers come from another process, so control characters and
/// stray quotes are possible and must not be able to break out of the string.
/// Non-ASCII is emitted as UTF-8 rather than `\u` escapes, which JSON allows
/// and which keeps a sidecar readable.
fn escape_into(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_keep_their_order() {
        let s = object(&[
            ("format", Value::str("public.rtf")),
            ("bytes", Value::Uint(42)),
            ("redacted", Value::Bool(true)),
            ("flavor", Value::Null),
        ]);
        assert_eq!(
            s,
            "{\n  \"format\": \"public.rtf\",\n  \"bytes\": 42,\n  \"redacted\": true,\n  \"flavor\": null\n}\n"
        );
    }

    #[test]
    fn hostile_identifiers_cannot_break_out_of_the_string() {
        let s = object(&[("format", Value::str("a\"b\\c\nd\u{1}e"))]);
        assert_eq!(s, "{\n  \"format\": \"a\\\"b\\\\c\\nd\\u0001e\"\n}\n");
    }

    #[test]
    fn non_ascii_stays_utf8() {
        let s = object(&[("app", Value::str("Café"))]);
        assert!(s.contains("\"Café\""));
    }
}

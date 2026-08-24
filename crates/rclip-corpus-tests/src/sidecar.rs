//! The sidecar contract from `corpus/README.md`, as code.

use crate::json::{self, Object, Value};
use std::fmt;

/// Keys the contract knows about.
///
/// Anything else, unless it starts with `expect_`, is rejected — a sidecar with
/// an `"expct"` key documents nothing and no test would ever notice. Crate
/// specific expectations (`expect_fragment`, `expect_source_url`, …) get the
/// `expect_` prefix precisely so that this list does not have to grow every
/// time a codec wants to pin one more value.
pub const KNOWN_KEYS: &[&str] = &[
    // Required of every fixture.
    "format",
    "origin",
    "description",
    "expect",
    // Universal optional.
    "notes",
    "error_kind",
    // Required of a captured fixture.
    "os",
    "app",
    "how",
    // Optional capture provenance.
    "os_version",
    "app_version",
    "captured_at",
    "redacted",
    // What the payload was offered as, and how big it was: `rclip-core`'s
    // `Flavor` name and a byte count, written by the capture tool.
    "flavor",
    "bytes",
    // Which `NSPasteboardItem` a multi-item capture came from.
    "item",
    // The leak scanner's escape hatch.
    "leak_allow",
    "leak_allow_reason",
];

/// Largest a fixture may be. These are unit-test inputs; the biggest thing in
/// the corpus today is a 2.5 KB shell link, so this is two orders of magnitude
/// of headroom before a stray capture starts bloating the repository.
pub const MAX_FIXTURE_BYTES: u64 = 256 * 1024;

/// Largest a sidecar may be. `notes` is prose, not an attachment.
pub const MAX_SIDECAR_BYTES: u64 = 64 * 1024;

/// `"synthetic"` or `"captured"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Hand-built bytes.
    Synthetic,
    /// Cut from a real machine.
    Captured,
}

/// `"ok"` or `"error"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// The parser must accept it.
    Ok,
    /// The parser must reject it, with a named kind.
    Error,
}

/// A validated sidecar.
#[derive(Debug, Clone)]
pub struct Sidecar {
    /// Every key, as read.
    pub raw: Object,
    /// `format`.
    pub format: String,
    /// `origin`.
    pub origin: Origin,
    /// `description`.
    pub description: String,
    /// `expect`.
    pub expect: Expect,
    /// `notes`, if any.
    pub notes: Option<String>,
    /// The `ErrorKind` variant name an `"error"` fixture must produce, and
    /// where that came from.
    pub error_kind: Option<DeclaredKind>,
    /// `redacted: true`.
    pub redacted: bool,
    /// Leak-scanner rules this fixture is excused from, from `leak_allow`.
    pub leak_allow: Vec<String>,
}

/// An `ErrorKind` named by a sidecar, and how it was named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredKind {
    /// Canonical variant name, e.g. `BadOffset`.
    pub variant: String,
    /// The key it was read from, or `"notes"`.
    pub source: String,
}

/// Every way a sidecar can fail the contract.
#[derive(Debug, Clone)]
pub struct Problem {
    /// Which key, where one is to blame.
    pub key: Option<String>,
    /// What is wrong, and what to do about it.
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.key {
            Some(k) => write!(f, "\"{k}\": {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

fn problem(key: &str, message: impl Into<String>) -> Problem {
    Problem {
        key: Some(key.to_owned()),
        message: message.into(),
    }
}

/// Every `ErrorKind` in `rclip-core`, by variant name and by the human string
/// `ErrorKind::as_str` returns.
///
/// Both spellings are in the corpus already — `rclip-idlist` writes
/// `"BadLength"`, `rclip-rtf` writes `"bad length field"` — and both crates'
/// own tests depend on the spelling they chose, so this accepts either rather
/// than rewriting sidecars other tests read.
const ERROR_KINDS: &[(&str, &str)] = &[
    ("UnexpectedEof", "unexpected end of input"),
    ("BadMagic", "bad magic"),
    ("BadLength", "bad length field"),
    ("BadOffset", "bad offset field"),
    ("Unsupported", "unsupported construct"),
    ("InvalidUtf8", "invalid UTF-8"),
    ("InvalidUtf16", "invalid UTF-16"),
    ("DepthLimit", "nesting depth limit exceeded"),
    ("TooLarge", "declared size too large"),
    ("Malformed", "malformed"),
];

/// Resolve either spelling of a kind to its variant name.
#[must_use]
pub fn canonical_kind(s: &str) -> Option<&'static str> {
    let s = s.trim();
    ERROR_KINDS
        .iter()
        .find(|(variant, human)| s.eq_ignore_ascii_case(variant) || s.eq_ignore_ascii_case(human))
        .map(|(variant, _)| *variant)
}

/// Pull an `ErrorKind::Foo` out of sidecar prose.
///
/// Only the qualified spelling counts. A bare `Malformed` in a sentence is
/// prose; `ErrorKind::Malformed` is a claim, and this gate holds the fixture to
/// it. If two different kinds are named the sidecar is contradicting itself and
/// this reports it rather than picking one.
fn kind_from_notes(notes: &str) -> Result<Option<&'static str>, Problem> {
    let mut found: Option<&'static str> = None;
    let mut rest = notes;
    while let Some(at) = rest.find("ErrorKind::") {
        rest = &rest[at + "ErrorKind::".len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        let Some(variant) = canonical_kind(&rest[..end]) else {
            continue;
        };
        match found {
            Some(prev) if prev != variant => {
                return Err(Problem {
                    key: Some("notes".into()),
                    message: format!(
                        "names two different kinds ({prev} and {variant}); \
                         set \"error_kind\" to the one the parser must return"
                    ),
                })
            }
            _ => found = Some(variant),
        }
        rest = &rest[end..];
    }
    Ok(found)
}

fn string_field(o: &Object, key: &str, out: &mut Vec<Problem>) -> Option<String> {
    match o.get(key) {
        Some(Value::Str(s)) if !s.trim().is_empty() => Some(s.clone()),
        Some(Value::Str(_)) => {
            out.push(problem(key, "is empty"));
            None
        }
        Some(v) => {
            out.push(problem(
                key,
                format!("must be a string, not {}", v.type_name()),
            ));
            None
        }
        None => {
            out.push(problem(key, "is required"));
            None
        }
    }
}

/// Read and validate one sidecar.
///
/// # Errors
///
/// Every problem found, not just the first: fixing sidecars one failed
/// assertion at a time is how a corpus sweep becomes a chore nobody runs.
pub fn parse(text: &str) -> Result<Sidecar, Vec<Problem>> {
    let raw = match json::parse_object(text) {
        Ok(o) => o,
        Err(e) => {
            return Err(vec![Problem {
                key: None,
                message: format!("is not valid JSON: {e}"),
            }])
        }
    };
    let mut problems = Vec::new();

    for key in raw.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) && !key.starts_with("expect_") {
            problems.push(problem(
                key,
                "is not a contract key; per-codec expectations take an \"expect_\" prefix",
            ));
        }
    }

    let format = string_field(&raw, "format", &mut problems);
    let description = string_field(&raw, "description", &mut problems);

    let origin = match string_field(&raw, "origin", &mut problems).as_deref() {
        Some("synthetic") => Some(Origin::Synthetic),
        Some("captured") => Some(Origin::Captured),
        Some(other) => {
            problems.push(problem(
                "origin",
                format!("is {other:?}; must be \"synthetic\" or \"captured\""),
            ));
            None
        }
        None => None,
    };

    let expect = match string_field(&raw, "expect", &mut problems).as_deref() {
        Some("ok") => Some(Expect::Ok),
        Some("error") => Some(Expect::Error),
        Some(other) => {
            problems.push(problem(
                "expect",
                format!("is {other:?}; must be \"ok\" or \"error\""),
            ));
            None
        }
        None => None,
    };

    // A capture has to be repeatable, or it is just bytes with a story.
    if origin == Some(Origin::Captured) {
        for key in ["os", "app", "how"] {
            if !matches!(raw.get(key), Some(Value::Str(s)) if !s.trim().is_empty()) {
                problems.push(problem(
                    key,
                    "is required of a \"captured\" fixture, so the capture can be repeated",
                ));
            }
        }
    }

    let notes = raw.get("notes").and_then(|v| v.as_str()).map(str::to_owned);

    // Explicit key beats prose, and `expect_error_kind` is the spelling
    // `rclip-cf-html` already writes.
    let mut error_kind = None;
    for key in ["error_kind", "expect_error_kind"] {
        let Some(v) = raw.get(key) else { continue };
        match v.as_str().and_then(canonical_kind) {
            Some(variant) => {
                error_kind = Some(DeclaredKind {
                    variant: variant.to_owned(),
                    source: key.to_owned(),
                });
                break;
            }
            None => problems.push(problem(
                key,
                format!(
                    "is {v:?}, which is not an rclip_core::ErrorKind; \
                     write the variant name or the string ErrorKind::as_str returns"
                ),
            )),
        }
    }
    if error_kind.is_none() {
        match notes.as_deref().map(kind_from_notes).transpose() {
            Ok(Some(Some(variant))) => {
                error_kind = Some(DeclaredKind {
                    variant: variant.to_owned(),
                    source: "notes".to_owned(),
                });
            }
            Ok(_) => {}
            Err(p) => problems.push(p),
        }
    }

    if expect == Some(Expect::Error) && error_kind.is_none() {
        problems.push(problem(
            "error_kind",
            "an \"error\" fixture must name the ErrorKind it produces — \
             \"returns some error\" is not the requirement, \"returns this error\" is",
        ));
    }
    if expect == Some(Expect::Ok) {
        if let Some(k) = &error_kind {
            if k.source != "notes" {
                problems.push(problem(
                    &k.source,
                    "names an ErrorKind on a fixture that is expected to parse",
                ));
            }
        }
    }

    let redacted = match raw.get("redacted") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(v) => {
            problems.push(problem(
                "redacted",
                format!("must be true or false, not {}", v.type_name()),
            ));
            false
        }
    };

    let mut leak_allow = Vec::new();
    if let Some(v) = raw.get("leak_allow") {
        match v.as_str() {
            Some(s) => {
                leak_allow = s
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                if leak_allow.is_empty() {
                    problems.push(problem("leak_allow", "lists no rules"));
                }
            }
            None => problems.push(problem(
                "leak_allow",
                "must be a comma-separated list of leak-scanner rule names",
            )),
        }
        if !matches!(raw.get("leak_allow_reason"), Some(Value::Str(s)) if !s.trim().is_empty()) {
            problems.push(problem(
                "leak_allow_reason",
                "is required beside \"leak_allow\": an allowlist entry with no reason is one \
                 nobody can ever safely remove",
            ));
        }
    } else if raw.contains_key("leak_allow_reason") {
        problems.push(problem(
            "leak_allow_reason",
            "has no \"leak_allow\" to explain",
        ));
    }

    if !problems.is_empty() {
        return Err(problems);
    }

    Ok(Sidecar {
        format: format.expect("checked"),
        origin: origin.expect("checked"),
        description: description.expect("checked"),
        expect: expect.expect("checked"),
        notes,
        error_kind,
        redacted,
        leak_allow,
        raw,
    })
}

#[cfg(test)]
mod tests {
    use super::{canonical_kind, parse, Expect, Origin};

    const OK: &str = r#"{
      "format": "CF_HDROP",
      "origin": "synthetic",
      "description": "d",
      "expect": "ok",
      "notes": "n"
    }"#;

    #[test]
    fn a_well_formed_sidecar_passes() {
        let s = parse(OK).unwrap();
        assert_eq!(s.origin, Origin::Synthetic);
        assert_eq!(s.expect, Expect::Ok);
        assert!(s.error_kind.is_none());
    }

    #[test]
    fn every_required_key_is_reported_at_once() {
        let problems = parse(r#"{"notes": "n"}"#).unwrap_err();
        let text = problems
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        for key in ["format", "origin", "description", "expect"] {
            assert!(text.contains(key), "{text}");
        }
    }

    #[test]
    fn a_capture_must_say_how_it_was_taken() {
        let src = OK.replace(r#""origin": "synthetic""#, r#""origin": "captured""#);
        let problems = parse(&src).unwrap_err();
        let text = problems
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        for key in ["os", "app", "how"] {
            assert!(text.contains(key), "{text}");
        }
    }

    #[test]
    fn an_error_fixture_without_a_kind_is_refused() {
        let src = OK.replace(r#""expect": "ok""#, r#""expect": "error""#);
        assert!(parse(&src).is_err());
    }

    #[test]
    fn a_kind_is_read_out_of_prose_or_out_of_a_key() {
        let from_notes = OK
            .replace(r#""expect": "ok""#, r#""expect": "error""#)
            .replace(
                r#""notes": "n""#,
                r#""notes": "Expect ErrorKind::BadOffset here.""#,
            );
        let k = parse(&from_notes).unwrap().error_kind.unwrap();
        assert_eq!(k.variant, "BadOffset");
        assert_eq!(k.source, "notes");

        let from_key = OK
            .replace(r#""expect": "ok""#, r#""expect": "error""#)
            .replace(r#""notes": "n""#, r#""error_kind": "bad offset field""#);
        assert_eq!(
            parse(&from_key).unwrap().error_kind.unwrap().variant,
            "BadOffset"
        );
    }

    #[test]
    fn prose_naming_two_kinds_is_a_contradiction() {
        let src = OK
            .replace(r#""expect": "ok""#, r#""expect": "error""#)
            .replace(
                r#""notes": "n""#,
                r#""notes": "ErrorKind::BadOffset, or maybe ErrorKind::TooLarge""#,
            );
        assert!(parse(&src).is_err());
    }

    #[test]
    fn a_bare_variant_name_in_prose_is_prose() {
        // "Must be UnexpectedEof" is a sentence, not a declaration; only the
        // qualified spelling binds.
        let src = OK
            .replace(r#""expect": "ok""#, r#""expect": "error""#)
            .replace(r#""notes": "n""#, r#""notes": "Must be UnexpectedEof.""#);
        assert!(parse(&src).is_err());
    }

    #[test]
    fn a_typo_key_is_caught_but_expect_prefixed_ones_are_not() {
        let typo = OK.replace(r#""notes": "n""#, r#""expct": "ok""#);
        assert!(parse(&typo).is_err());
        let pinned = OK.replace(r#""notes": "n""#, r#""expect_fragment": "<p>x</p>""#);
        assert!(parse(&pinned).is_ok());
    }

    #[test]
    fn an_allowlist_needs_a_reason() {
        let no_reason = OK.replace(r#""notes": "n""#, r#""leak_allow": "HomePath""#);
        assert!(parse(&no_reason).is_err());
        let with_reason = OK.replace(
            r#""notes": "n""#,
            r#""leak_allow": "HomePath", "leak_allow_reason": "because""#,
        );
        assert_eq!(parse(&with_reason).unwrap().leak_allow, ["HomePath"]);
    }

    #[test]
    fn both_spellings_of_a_kind_resolve() {
        assert_eq!(canonical_kind("TooLarge"), Some("TooLarge"));
        assert_eq!(canonical_kind("declared size too large"), Some("TooLarge"));
        assert_eq!(canonical_kind("toolarge"), Some("TooLarge"));
        assert_eq!(canonical_kind("nope"), None);
    }
}

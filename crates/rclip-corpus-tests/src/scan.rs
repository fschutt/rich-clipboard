//! The leak scanner.
//!
//! A capture is cut from a real machine, and a real machine is full of things
//! that must not end up in a public repository: volume UUIDs, sandbox tokens,
//! home directory paths, a volume named after its owner. One such capture has
//! already had to be scrubbed by hand in this corpus. Reading a report caught
//! it; a check should.
//!
//! # False positives are the failure mode
//!
//! A scanner that cries wolf gets switched off, which is strictly worse than
//! not having one. Every rule here is therefore written around a *shape* that
//! synthetic test data does not naturally take, with the obvious placeholders
//! spelled out as allowed:
//!
//! - **`Uuid`** — the nil UUID is the redaction placeholder, and the OLE
//!   reserved block `{________-____-____-C000-000000000046}` is COM's own
//!   published identifiers, not anybody's machine.
//! - **`HexRun`** — a run of one repeated digit is a mask or a placeholder. No
//!   real key or token is thirty-two identical characters.
//! - **`HomePath`** — a documented set of placeholder user names (`me`,
//!   `example`, `testuser`, …) is what a hand-built fixture uses, so only a
//!   name outside that set is worth a human's attention.
//! - **`Email`** — the RFC 2606 reserved domains (`example.com`, `.test`,
//!   `.invalid`, `.localhost`) are what documentation uses.
//! - **`CurrentUser`** — derived from the environment, and skipped entirely
//!   when the login name is a generic one (`root`, `runner`, `ubuntu`), because
//!   matching a CI runner's user name against fixture bytes is all noise.
//! - **`PersonalDevice`** — a personalised volume name cannot be enumerated,
//!   so it is caught by the shape personalisation takes: a capitalised name, a
//!   possessive, and a piece of hardware. `Macintosh HD` never matches that;
//!   `Someone's MacBook Pro` always does.
//!
//! Where a fixture legitimately trips a rule anyway, its sidecar says so with
//! `leak_allow` plus a `leak_allow_reason`.

use std::fmt;

/// Which rule fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// The name or home directory of whoever ran the scan.
    CurrentUser,
    /// A path under `/Users/`, `/home/` or `\Users\` naming somebody.
    HomePath,
    /// A UUID that is not a placeholder or a published constant.
    Uuid,
    /// Thirty-two or more hex characters in a row.
    HexRun,
    /// An email address outside the reserved documentation domains.
    Email,
    /// A possessive device or volume name.
    PersonalDevice,
}

impl Rule {
    /// The name used in a sidecar's `leak_allow`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::CurrentUser => "CurrentUser",
            Self::HomePath => "HomePath",
            Self::Uuid => "Uuid",
            Self::HexRun => "HexRun",
            Self::Email => "Email",
            Self::PersonalDevice => "PersonalDevice",
        }
    }

    /// Every rule, for documenting the allowlist vocabulary.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::CurrentUser,
            Self::HomePath,
            Self::Uuid,
            Self::HexRun,
            Self::Email,
            Self::PersonalDevice,
        ]
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How the match was spelled in the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Plain bytes.
    Ascii,
    /// Every other byte a NUL — these formats are full of UTF-16.
    Utf16Le,
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ascii => "ASCII",
            Self::Utf16Le => "UTF-16LE",
        })
    }
}

/// One thing that must not be in a public repository.
#[derive(Debug, Clone)]
pub struct Finding {
    /// Which rule fired.
    pub rule: Rule,
    /// Byte offset into the buffer that was scanned.
    pub offset: usize,
    /// How it was encoded.
    pub encoding: Encoding,
    /// The matched text, printable-escaped.
    pub matched: String,
    /// What to do about it.
    pub why: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at byte {} ({}): {:?} — {}",
            self.rule, self.offset, self.encoding, self.matched, self.why
        )
    }
}

/// Who is running the scan, derived at run time.
///
/// Nothing here is hardcoded: the check has to work for every contributor, and
/// a scanner that only knows one person's name is a scanner that passes for
/// everybody else.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    /// Login name, unless it is one every CI runner shares.
    pub login: Option<String>,
    /// Home directory path.
    pub home: Option<String>,
    /// Last component of the home directory, when it differs from the login.
    pub home_base: Option<String>,
}

/// Login names that say nothing about a person. Matching these against fixture
/// bytes produces noise and no signal, so the `CurrentUser` rule sits them out;
/// `HomePath` still covers a path that names one.
const GENERIC_LOGINS: &[&str] = &[
    "root",
    "runner",
    "ubuntu",
    "user",
    "admin",
    "administrator",
    "build",
    "builder",
    "ci",
    "docker",
    "vagrant",
    "jenkins",
    "nobody",
    "me",
    "test",
    "guest",
    "container",
    "codespace",
    "vscode",
    "app",
    "node",
];

/// User names a hand-built fixture uses. A path under one of these is a
/// stand-in, not somebody's home directory.
const PLACEHOLDER_USERS: &[&str] = &[
    "me",
    "user",
    "username",
    "someone",
    "somebody",
    "anon",
    "anonymous",
    "example",
    "test",
    "tester",
    "testuser",
    "demo",
    "dev",
    "developer",
    "foo",
    "bar",
    "baz",
    "alice",
    "bob",
    "carol",
    "eve",
    "jdoe",
    "johndoe",
    "john",
    "jane",
    "nobody",
    "root",
    "runner",
    "ubuntu",
    "vagrant",
    "docker",
    "guest",
    "build",
    "ci",
];

/// Domains reserved for documentation and testing (RFC 2606, RFC 6761).
const RESERVED_EMAIL_DOMAINS: &[&str] =
    &["example.com", "example.net", "example.org", "example.edu"];
const RESERVED_EMAIL_TLDS: &[&str] = &["example", "test", "invalid", "localhost", "local"];

/// Published COM identifiers that show up in shell formats.
const WELL_KNOWN_UUIDS: &[&str] = &[
    // CLSID_ShellLink and the ITEMIDLIST root folders that name it.
    "00021401-0000-0000-c000-000000000046",
    "20d04fe0-3aea-1069-a2d8-08002b30309d",
    "21ec2020-3aea-1069-a2dd-08002b30309d",
    "450d8fba-ad25-11d0-98a8-0800361b1103",
    "645ff040-5081-101b-9f08-00aa002f954e",
];

/// The OLE reserved block: every `{________-____-____-C000-000000000046}` is a
/// published Microsoft interface or class identifier.
const OLE_SUFFIX: &str = "-c000-000000000046";

/// A redaction placeholder: every hex digit the same, apart from the two
/// nibbles that say what kind of UUID it is.
///
/// The nil UUID is the placeholder `corpus/README.md` names, but a scrubber is
/// right to keep the version and variant nibbles when the original was a v4 —
/// `00000000-0000-4000-8000-000000000000` is still a structurally valid v4
/// UUID, which matters to anything that validates one, and it carries no more
/// information than the nil UUID does. Digit 12 is the version, digit 16 the
/// variant; every other one has to be uniform, and a real identifier is never
/// thirty zeros with two nibbles set.
fn is_placeholder_uuid(lower: &str) -> bool {
    let digits: Vec<u8> = lower.bytes().filter(|&b| b != b'-').collect();
    if digits.len() != 32 {
        return false;
    }
    let uniform = |fill: u8| {
        digits
            .iter()
            .enumerate()
            .all(|(i, &d)| d == fill || i == 12 || i == 16)
    };
    uniform(b'0') || uniform(b'f')
}

/// Hardware and volume words that follow a possessive in a personalised name.
const DEVICE_WORDS: &[&str] = &[
    "mac",
    "macbook",
    "imac",
    "iphone",
    "ipad",
    "ipod",
    "macintosh",
    "pc",
    "computer",
    "laptop",
    "desktop",
    "ssd",
    "drive",
    "disk",
    "volume",
    "backup",
    "time",
    "airport",
];

/// Words that take a possessive in ordinary prose. Combined with the
/// capitalised-possessor rule, this is what keeps "the user's disk" in a
/// fixture's body text from reading as somebody's volume name.
const COMMON_POSSESSORS: &[&str] = &[
    "the",
    "a",
    "an",
    "user",
    "users",
    "it",
    "one",
    "system",
    "everyone",
    "someone",
    "anyone",
    "today",
    "apple",
    "microsoft",
    "windows",
    "finder",
    "nautilus",
    "explorer",
    "word",
    "excel",
    "chrome",
    "safari",
    "firefox",
    "qt",
    "gtk",
    "kde",
    "gnome",
    "office",
    "outlook",
    "author",
    "owner",
    "machine",
    "host",
    "client",
    "server",
    "parser",
    "reader",
    "writer",
    "caller",
];

/// Derive the running user's identity from the environment.
#[must_use]
pub fn identity_from_env() -> Identity {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let login = var("USER")
        .or_else(|| var("LOGNAME"))
        .or_else(|| var("USERNAME"));
    let home = var("HOME").or_else(|| var("USERPROFILE"));
    let home_base = home
        .as_deref()
        .and_then(|h| h.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let usable = |name: &String| {
        // Two characters cannot be matched without swamping the report, and a
        // shared CI login is not an identity.
        name.len() >= 3 && !GENERIC_LOGINS.contains(&name.to_ascii_lowercase().as_str())
    };
    Identity {
        login: login.filter(usable),
        home_base: home_base.filter(usable),
        home: home.filter(|h| h.len() >= 4 && h != "/" && h != "/root"),
    }
}

/// Scan a buffer, both as bytes and as UTF-16LE.
#[must_use]
pub fn scan(buf: &[u8], id: &Identity) -> Vec<Finding> {
    let mut out = scan_view(buf, id, Encoding::Ascii, None);
    // Both alignments: a UTF-16 string does not have to start on an even byte
    // of the payload it is embedded in, and in these formats it routinely does
    // not.
    for align in [0usize, 1] {
        let (text, offsets) = utf16le_view(buf, align);
        out.extend(scan_view(
            text.as_bytes(),
            id,
            Encoding::Utf16Le,
            Some(&offsets),
        ));
    }
    dedup(out)
}

/// Scan a decoded sidecar value. Text only — sidecars are UTF-8 prose.
#[must_use]
pub fn scan_text(text: &str, id: &Identity) -> Vec<Finding> {
    dedup(scan_view(text.as_bytes(), id, Encoding::Ascii, None))
}

fn dedup(v: Vec<Finding>) -> Vec<Finding> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for f in v {
        if seen.insert((f.rule, f.offset, f.matched.clone())) {
            out.push(f);
        }
    }
    out.sort_by_key(|f| (f.offset, f.rule));
    out
}

/// Project the UTF-16LE-looking parts of `buf` onto one byte per code unit.
///
/// A code unit whose high byte is zero and whose low byte is printable becomes
/// that character; U+2019 becomes a plain apostrophe so the possessive rule
/// sees it; everything else becomes a NUL, which no rule matches across. The
/// returned vector maps each projected byte back to its offset in `buf`, so a
/// finding still names a real offset in the real file.
fn utf16le_view(buf: &[u8], align: usize) -> (String, Vec<usize>) {
    let mut text = Vec::with_capacity(buf.len() / 2);
    let mut offsets = Vec::with_capacity(buf.len() / 2);
    let mut i = align;
    while i + 1 < buf.len() {
        let (lo, hi) = (buf[i], buf[i + 1]);
        let c = match (lo, hi) {
            (0x20..=0x7e, 0x00) => lo,
            // RIGHT SINGLE QUOTATION MARK, the apostrophe macOS actually uses.
            (0x19, 0x20) => b'\'',
            _ => 0,
        };
        text.push(c);
        offsets.push(i);
        i += 2;
    }
    // Every byte pushed is either NUL or printable ASCII, so this is UTF-8.
    (String::from_utf8(text).unwrap_or_default(), offsets)
}

fn map(offsets: Option<&[usize]>, i: usize) -> usize {
    offsets.map_or(i, |o| o.get(i).copied().unwrap_or(i))
}

fn scan_view(
    buf: &[u8],
    id: &Identity,
    encoding: Encoding,
    offsets: Option<&[usize]>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<Finding>, rule, at, matched: String, why: String| {
        out.push(Finding {
            rule,
            offset: map(offsets, at),
            encoding,
            matched,
            why,
        });
    };

    // --- CurrentUser -------------------------------------------------------
    if let Some(home) = &id.home {
        for at in find_all_ci(buf, home.as_bytes()) {
            push(
                &mut out,
                Rule::CurrentUser,
                at,
                home.clone(),
                "this is the home directory of whoever ran the scan; scrub it in place at the \
                 same byte length"
                    .to_owned(),
            );
        }
    }
    for name in [id.login.as_ref(), id.home_base.as_ref()]
        .into_iter()
        .flatten()
    {
        for at in find_all_ci(buf, name.as_bytes()) {
            if !is_word_bounded(buf, at, name.len()) {
                continue;
            }
            push(
                &mut out,
                Rule::CurrentUser,
                at,
                name.clone(),
                "this is the login name of whoever ran the scan".to_owned(),
            );
        }
    }

    // --- HomePath ----------------------------------------------------------
    for (at, prefix, user) in home_paths(buf) {
        let lower = user.to_ascii_lowercase();
        if PLACEHOLDER_USERS.contains(&lower.as_str()) {
            continue;
        }
        // A path naming the running user is reported under CurrentUser, which
        // no fixture may allowlist away.
        let rule = if id
            .login
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case(&user))
        {
            Rule::CurrentUser
        } else {
            Rule::HomePath
        };
        push(
            &mut out,
            rule,
            at,
            format!("{prefix}{user}"),
            format!(
                "{user:?} is not one of the placeholder user names \
                 ({}); if this is synthetic, use one of those, otherwise scrub it",
                PLACEHOLDER_USERS[..6].join(", ")
            ),
        );
    }

    // --- Uuid --------------------------------------------------------------
    for (at, text) in uuids(buf) {
        let lower = text.to_ascii_lowercase();
        if is_placeholder_uuid(&lower)
            || WELL_KNOWN_UUIDS.contains(&lower.as_str())
            || lower.ends_with(OLE_SUFFIX)
        {
            continue;
        }
        push(
            &mut out,
            Rule::Uuid,
            at,
            text,
            "a UUID identifies a volume, a machine or an install; replace it with the nil UUID, \
             which is the same length"
                .to_owned(),
        );
    }

    // --- HexRun ------------------------------------------------------------
    for (at, text) in hex_runs(buf) {
        if text.bytes().all(|b| b == text.as_bytes()[0]) {
            continue;
        }
        let shown = format!("{}…{}", &text[..8], &text[text.len() - 8..]);
        push(
            &mut out,
            Rule::HexRun,
            at,
            shown,
            format!(
                "{} hex characters in a row is the shape of a key, a token or a hash; \
                 replace it with the same number of zeros",
                text.len()
            ),
        );
    }

    // --- Email -------------------------------------------------------------
    for (at, text) in emails(buf) {
        let domain = text.rsplit('@').next().unwrap_or("").to_ascii_lowercase();
        let tld = domain.rsplit('.').next().unwrap_or("");
        if RESERVED_EMAIL_DOMAINS.contains(&domain.as_str())
            || RESERVED_EMAIL_TLDS.contains(&tld)
            || domain == "localhost"
        {
            continue;
        }
        push(
            &mut out,
            Rule::Email,
            at,
            text,
            "use a reserved documentation domain (example.com, .test, .invalid) instead".to_owned(),
        );
    }

    // --- PersonalDevice ----------------------------------------------------
    for (at, text) in possessive_devices(buf) {
        push(
            &mut out,
            Rule::PersonalDevice,
            at,
            text,
            "a volume or device named after its owner; \"Macintosh HD\" is the default name and \
             is fine, this is not"
                .to_owned(),
        );
    }

    out
}

// --------------------------------------------------------------------------
// Matchers. No regex: the patterns are fixed and a dependency-free workspace
// is worth more than four lines saved.
// --------------------------------------------------------------------------

fn find_all_ci(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    (0..=hay.len() - needle.len())
        .filter(|&i| {
            hay[i..i + needle.len()]
                .iter()
                .zip(needle)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
        .collect()
}

const fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_word_bounded(buf: &[u8], at: usize, len: usize) -> bool {
    let before = at.checked_sub(1).map(|i| buf[i]);
    let after = buf.get(at + len).copied();
    !before.is_some_and(is_word_byte) && !after.is_some_and(is_word_byte)
}

/// `/Users/<name>`, `/home/<name>`, `\Users\<name>`, in any case.
fn home_paths(buf: &[u8]) -> Vec<(usize, String, String)> {
    const PREFIXES: &[&str] = &["/Users/", "/home/", "\\Users\\", "/Home/", "\\home\\"];
    let mut out = Vec::new();
    for prefix in PREFIXES {
        for at in find_all_ci(buf, prefix.as_bytes()) {
            let start = at + prefix.len();
            let end = buf[start..]
                .iter()
                .position(
                    |&b| !matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'),
                )
                .map_or(buf.len(), |n| start + n);
            if end == start {
                continue; // A bare "/Users/" names nobody.
            }
            let user = String::from_utf8_lossy(&buf[start..end]).into_owned();
            out.push((at, (*prefix).to_owned(), user));
        }
    }
    out
}

const fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// `8-4-4-4-12` hex, not embedded in a longer hex-or-dash run.
fn uuids(buf: &[u8]) -> Vec<(usize, String)> {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    const LEN: usize = 36;
    let mut out = Vec::new();
    if buf.len() < LEN {
        return out;
    }
    'outer: for at in 0..=buf.len() - LEN {
        if at > 0 && (is_hex(buf[at - 1]) || buf[at - 1] == b'-') {
            continue;
        }
        if buf.get(at + LEN).is_some_and(|&b| is_hex(b) || b == b'-') {
            continue;
        }
        let mut i = at;
        for (g, &n) in GROUPS.iter().enumerate() {
            if g > 0 {
                if buf[i] != b'-' {
                    continue 'outer;
                }
                i += 1;
            }
            if !buf[i..i + n].iter().all(|&b| is_hex(b)) {
                continue 'outer;
            }
            i += n;
        }
        out.push((at, String::from_utf8_lossy(&buf[at..at + LEN]).into_owned()));
    }
    out
}

/// Maximal runs of thirty-two or more hex characters.
fn hex_runs(buf: &[u8]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        if !is_hex(buf[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < buf.len() && is_hex(buf[i]) {
            i += 1;
        }
        if i - start >= 32 {
            out.push((start, String::from_utf8_lossy(&buf[start..i]).into_owned()));
        }
    }
    out
}

fn emails(buf: &[u8]) -> Vec<(usize, String)> {
    const LOCAL: fn(u8) -> bool =
        |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'%' | b'+' | b'-');
    const DOMAIN: fn(u8) -> bool = |b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-');
    let mut out = Vec::new();
    for (at, _) in buf.iter().enumerate().filter(|(_, &b)| b == b'@') {
        let mut start = at;
        while start > 0 && LOCAL(buf[start - 1]) {
            start -= 1;
        }
        let mut end = at + 1;
        while end < buf.len() && DOMAIN(buf[end]) {
            end += 1;
        }
        if start == at || end == at + 1 {
            continue;
        }
        // A domain with no dot is a local alias, not an address; a TLD that is
        // not letters is a version number or a file name.
        let domain = &buf[at + 1..end];
        let Some(dot) = domain.iter().rposition(|&b| b == b'.') else {
            continue;
        };
        let tld = &domain[dot + 1..];
        if !(2..=24).contains(&tld.len()) || !tld.iter().all(u8::is_ascii_alphabetic) {
            continue;
        }
        if buf[start] == b'.' || buf[at - 1] == b'.' || domain[0] == b'.' {
            continue;
        }
        out.push((
            start,
            String::from_utf8_lossy(&buf[start..end]).into_owned(),
        ));
    }
    out
}

/// `Name's Device` — a capitalised possessor, an apostrophe, `s`, a hardware
/// word.
fn possessive_devices(buf: &[u8]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for at in 0..buf.len() {
        // Apostrophe: ASCII, or UTF-8 U+2019, or UTF-8 U+00B4.
        let apos_len = if buf[at] == b'\'' {
            1
        } else if buf[at..].starts_with(b"\xe2\x80\x99") {
            3
        } else if buf[at..].starts_with(b"\xc2\xb4") {
            2
        } else {
            continue;
        };
        let after = at + apos_len;
        if !matches!(buf.get(after), Some(b's' | b'S')) {
            continue;
        }
        if !matches!(buf.get(after + 1), Some(b' ')) {
            continue;
        }
        // The possessor: letters immediately before the apostrophe.
        let mut start = at;
        while start > 0 && (buf[start - 1].is_ascii_alphanumeric() || buf[start - 1] >= 0x80) {
            start -= 1;
        }
        let possessor = String::from_utf8_lossy(&buf[start..at]).into_owned();
        if possessor.chars().count() < 2 || !possessor.starts_with(|c: char| c.is_uppercase()) {
            continue;
        }
        if COMMON_POSSESSORS.contains(&possessor.to_ascii_lowercase().as_str()) {
            continue;
        }
        // The device word.
        let word_start = after + 2;
        let word_end = buf[word_start..]
            .iter()
            .position(|b| !b.is_ascii_alphanumeric())
            .map_or(buf.len(), |n| word_start + n);
        let word = String::from_utf8_lossy(&buf[word_start..word_end]).to_ascii_lowercase();
        if !DEVICE_WORDS.contains(&word.as_str()) {
            continue;
        }
        // Include one following word, so "Someone's MacBook Pro" reads whole.
        let mut end = word_end;
        if buf.get(end) == Some(&b' ') {
            let next = end + 1;
            let next_end = buf[next..]
                .iter()
                .position(|b| !b.is_ascii_alphanumeric())
                .map_or(buf.len(), |n| next + n);
            if next_end > next && buf[next].is_ascii_uppercase() {
                end = next_end;
            }
        }
        out.push((
            start,
            String::from_utf8_lossy(&buf[start..end]).into_owned(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{scan, scan_text, Encoding, Identity, Rule};

    fn id() -> Identity {
        Identity {
            login: Some("acontributor".into()),
            home: Some("/Users/acontributor".into()),
            home_base: Some("acontributor".into()),
        }
    }

    fn rules(findings: &[super::Finding]) -> Vec<Rule> {
        let mut v: Vec<_> = findings.iter().map(|f| f.rule).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn wide(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn the_nil_uuid_is_the_placeholder_and_passes() {
        let f = scan(b"vol 00000000-0000-0000-0000-000000000000 end", &id());
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn a_scrubbed_uuid_may_keep_its_version_and_variant_nibbles() {
        // Zeroed but still a structurally valid v4, which is what a scrubber
        // should write when the original was one.
        for ok in [
            "00000000-0000-4000-8000-000000000000",
            "00000000-0000-1000-9000-000000000000",
            "ffffffff-ffff-4fff-bfff-ffffffffffff",
        ] {
            assert!(scan(ok.as_bytes(), &id()).is_empty(), "{ok}");
        }
        // One more non-zero nibble and it is an identifier again.
        let f = scan(b"00000000-0000-4000-8000-000000000001", &id());
        assert_eq!(rules(&f), [Rule::Uuid]);
    }

    #[test]
    fn a_real_uuid_is_caught_in_ascii_and_in_utf16() {
        let uuid = "3F2504E0-4F89-11D3-9A0C-0305E82C3301";
        let ascii = scan(uuid.as_bytes(), &id());
        assert_eq!(rules(&ascii), [Rule::Uuid]);

        let utf16 = scan(&wide(uuid), &id());
        assert_eq!(rules(&utf16), [Rule::Uuid]);
        assert_eq!(utf16[0].encoding, Encoding::Utf16Le);
        assert_eq!(utf16[0].offset, 0);
    }

    #[test]
    fn a_utf16_uuid_at_an_odd_offset_is_still_found() {
        let mut buf = vec![0xAAu8];
        buf.extend(wide("3F2504E0-4F89-11D3-9A0C-0305E82C3301"));
        let f = scan(&buf, &id());
        assert_eq!(rules(&f), [Rule::Uuid]);
        assert_eq!(f[0].offset, 1, "the offset has to name the real byte");
    }

    #[test]
    fn published_com_identifiers_are_not_a_leak() {
        let f = scan(b"{000214A0-0000-0000-C000-000000000046}", &id());
        assert!(f.is_empty(), "{f:?}");
        let f = scan(b"20D04FE0-3AEA-1069-A2D8-08002B30309D", &id());
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn a_long_hex_token_is_caught_but_a_zero_run_is_not() {
        let zeros = "0".repeat(64);
        assert!(scan(zeros.as_bytes(), &id()).is_empty());
        let token = "a3f19c8e5b7d2461a3f19c8e5b7d2461";
        assert_eq!(rules(&scan(token.as_bytes(), &id())), [Rule::HexRun]);
        // Thirty-one is under the bar.
        assert!(scan(&token.as_bytes()[..31], &id()).is_empty());
    }

    #[test]
    fn placeholder_home_paths_pass_and_named_ones_do_not() {
        for ok in ["/Users/example/x", "/home/me/x", "C:\\Users\\testuser\\x"] {
            assert!(scan(ok.as_bytes(), &id()).is_empty(), "{ok}");
        }
        let f = scan(b"/Users/jbloggs/Documents/x.pdf", &id());
        assert_eq!(rules(&f), [Rule::HomePath]);
        assert!(f[0].matched.contains("jbloggs"));
    }

    #[test]
    fn the_running_users_own_path_is_reported_as_current_user() {
        let f = scan(b"file:///Users/acontributor/x", &id());
        assert!(f.iter().any(|f| f.rule == Rule::CurrentUser), "{f:?}");
    }

    #[test]
    fn a_login_name_only_matches_as_a_whole_word() {
        assert!(scan(b"reacontributoring", &id()).is_empty());
        assert_eq!(
            rules(&scan(b"owner: acontributor.", &id())),
            [Rule::CurrentUser]
        );
    }

    #[test]
    fn reserved_documentation_domains_are_not_addresses() {
        for ok in ["a@example.com", "x.y@sub.example", "q@host.invalid"] {
            assert!(scan(ok.as_bytes(), &id()).is_empty(), "{ok}");
        }
        assert_eq!(
            rules(&scan(b"contact j.bloggs@some-isp.de now", &id())),
            [Rule::Email]
        );
    }

    #[test]
    fn a_version_string_is_not_an_email() {
        assert!(scan(b"build@1.2.3", &id()).is_empty());
        assert!(scan(b"user@localhost", &id()).is_empty());
    }

    #[test]
    fn the_default_volume_name_is_fine_and_a_personal_one_is_not() {
        assert!(scan(b"Macintosh HD", &id()).is_empty());
        assert!(scan("Macintosh HD — Data".as_bytes(), &id()).is_empty());

        let f = scan("Jane\u{2019}s MacBook Pro".as_bytes(), &id());
        assert_eq!(rules(&f), [Rule::PersonalDevice]);
        assert_eq!(f[0].matched, "Jane\u{2019}s MacBook Pro");

        assert_eq!(
            rules(&scan(&wide("Bartholomew's iMac"), &id())),
            [Rule::PersonalDevice]
        );
    }

    #[test]
    fn ordinary_prose_does_not_read_as_a_device_name() {
        for ok in [
            "the user's disk is full",
            "The system's volume",
            "Windows' drive letter",
            "Apple's Mac",
        ] {
            assert!(scan(ok.as_bytes(), &id()).is_empty(), "{ok}");
        }
    }

    #[test]
    fn sidecar_prose_is_scanned_the_same_way() {
        let f = scan_text(
            "the original said /Users/jbloggs before it was scrubbed",
            &id(),
        );
        assert_eq!(rules(&f), [Rule::HomePath]);
    }

    #[test]
    fn every_rule_has_a_distinct_allowlist_name() {
        let mut names: Vec<_> = Rule::all().iter().map(|r| r.name()).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n);
    }
}

//! freedesktop `.desktop` entries — the Linux shortcut.
//!
//! Spec: [Desktop Entry Specification v1.5](https://specifications.freedesktop.org/desktop-entry/latest/).
//! Section numbers in the docs below refer to it.
//!
//! A `.desktop` file is an INI-shaped list of groups. `Type=Link` plus `URL=`
//! is the direct analogue of a Windows `.url` and a macOS `.webloc`, which is
//! why this crate sits in a clipboard workspace: dropping a launcher on an
//! application should yield structure, not a filename.
//!
//! # Nothing here runs anything
//!
//! `Exec=` is parsed into [`exec::ExecCommand`] — arguments and field codes as
//! data. No `$PATH` lookup, no field-code expansion, no process. A `.desktop`
//! file that arrived over the clipboard is attacker-controlled input;
//! `plan/CONVENTIONS.md` rule 6 names this format specifically.
//!
//! # The hard parts, and where they live
//!
//! - Escape sequences and `\;` in list values — [`value`], which explains why
//!   splitting has to happen before unescaping.
//! - Localized keys and the §5 fallback ladder — [`locale`].
//! - `Exec=`'s two stacked escape layers — [`exec`].
//!
//! # Example
//!
//! ```
//! # use rclip_desktop_entry::{parse, EntryType, Locale, ShortcutTarget};
//! let src = b"[Desktop Entry]\n\
//!             Type=Link\n\
//!             Name=Example\n\
//!             Name[de]=Beispiel\n\
//!             URL=https://example.com/\n";
//! let f = parse(src).unwrap();
//! assert_eq!(f.entry_type(), Some(EntryType::Link));
//! assert_eq!(f.target(), Some(ShortcutTarget::Url("https://example.com/")));
//!
//! let de = Locale::parse("de_DE.UTF-8").unwrap();
//! assert!(f.name(Some(&de)).unwrap().eq_str("Beispiel"));
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

pub mod exec;
mod lines;
pub mod locale;
pub mod shortcut;
pub mod value;

use rclip_core::{Error, ErrorKind, Reader, Result};

use lines::{Line, Lines};

pub use exec::{ExecArg, ExecArgs, ExecCommand, ExecPiece, ExecPieces, FieldCode};
pub use locale::Locale;
pub use shortcut::ShortcutTarget;
pub use value::{ListItems, Unescape, Value};

/// The group every desktop entry must have (§3.2).
pub const GROUP_DESKTOP_ENTRY: &str = "Desktop Entry";

/// Prefix of an action group (§11.1): `Desktop Action <id>`.
pub const GROUP_ACTION_PREFIX: &str = "Desktop Action ";

/// `Type=` (§6).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EntryType<'a> {
    /// `Type=Application` — a launcher, with `Exec=`.
    Application,
    /// `Type=Link` — a URL, with `URL=`. The `.url` / `.webloc` analogue.
    Link,
    /// `Type=Directory` — a menu directory.
    Directory,
    /// Anything else. §6: "To allow the addition of new types in the future,
    /// implementations should ignore desktop entries with an unknown type."
    /// Reported rather than rejected so the caller can decide to ignore it.
    Other(&'a str),
}

/// A parsed desktop entry file.
///
/// Borrows the caller's buffer. Nothing is unescaped, decoded or looked up
/// until an accessor asks.
#[derive(Debug, Copy, Clone)]
pub struct DesktopFile<'a> {
    src: &'a str,
    /// Offset of `src` within the original buffer — 3 if a UTF-8 BOM was
    /// stripped. All reported offsets are relative to the caller's buffer.
    base: usize,
}

/// Parse a `.desktop` file.
///
/// The structural rules checked here are the ones where continuing would mean
/// silently attributing data to the wrong place:
///
/// - the file is UTF-8 (§3);
/// - a `[` line closes with `]` and the name contains no `[`, `]` or control
///   character (§3.2);
/// - no `key=value` appears before the first group header — there would be no
///   group to file it under (§3.2: "There should be nothing preceding this
///   group in the desktop entry file but possibly one or more comments");
/// - every non-comment, non-blank line inside a group contains `=` (§3.3);
/// - key names use only `A-Za-z0-9-`, plus an optional `[LOCALE]` postfix
///   (§3.3, §5).
///
/// # Deliberately *not* checked
///
/// §3.2 forbids duplicate group names and §3.3 forbids duplicate keys within a
/// group. Detecting either is quadratic in the number of groups or keys, and
/// this parser's input arrives from another process — a payload with a hundred
/// thousand keys would turn an O(n²) uniqueness check into a hang. Lookups
/// therefore return the **first** match, which is what GLib does, and
/// duplicates are not diagnosed.
///
/// # Errors
///
/// [`ErrorKind::InvalidUtf8`] or [`ErrorKind::Malformed`], with the offset of
/// the offending line.
pub fn parse(bytes: &[u8]) -> Result<DesktopFile<'_>> {
    let (src, base) = decode(bytes)?;
    validate(src, base)?;
    Ok(DesktopFile { src, base })
}

/// Validate as UTF-8 and step over a byte-order mark.
///
/// §3 requires UTF-8 and says nothing about a BOM, but editors add one and
/// `\u{FEFF}[Desktop Entry]` does not start with `[` — without this the file
/// would parse as having no groups at all, which is indistinguishable from an
/// empty file.
fn decode(bytes: &[u8]) -> Result<(&str, usize)> {
    let mut r = Reader::new(bytes);
    let base = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        r.skip(3)?;
        3
    } else {
        0
    };
    let src = core::str::from_utf8(r.remaining())
        .map_err(|e| Error::new(ErrorKind::InvalidUtf8, base + e.valid_up_to()))?;
    Ok((src, base))
}

/// The group name in `[Name]`, or `None` if the line is not a header.
///
/// Leading whitespace is tolerated. §3.2 does not allow it, but GLib's
/// `g_key_file_parse_line` skips leading whitespace before deciding what a line
/// is, and GLib is the implementation every `.desktop` file in the wild has
/// actually been tested against.
fn header_of(text: &str) -> Option<&str> {
    let t = text.trim();
    let inner = t.strip_prefix('[')?.strip_suffix(']')?;
    // §3.2: "Group names may contain all ASCII characters except for `[` and
    // `]` and control characters." A `]` inside the name would make the header
    // ambiguous — `[a]b]` could be group `a]b` or group `a` followed by junk.
    if inner.is_empty() || inner.bytes().any(|b| b == b'[' || b == b']' || b.is_ascii_control()) {
        return None;
    }
    Some(inner)
}

/// `true` if the line is blank or a comment (§3.1).
///
/// Leading whitespace before `#` is accepted, for the same GLib reason as
/// [`header_of`].
fn is_skippable(text: &str) -> bool {
    let t = text.trim_start();
    t.trim_end().is_empty() || t.starts_with('#')
}

/// Split `Key` / `Key[LOCALE]` into its parts, validating the key charset.
///
/// Returns `None` when the key is not spelled the way §3.3 requires.
fn split_key(key: &str) -> Option<(&str, Option<&str>)> {
    let (base, locale) = match key.find('[') {
        Some(i) => {
            let l = key.get(i + 1..)?.strip_suffix(']')?;
            if l.is_empty() || l.contains(']') {
                return None;
            }
            (key.get(..i)?, Some(l))
        }
        None => (key, None),
    };
    if base.is_empty() || !base.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    Some((base, locale))
}

fn validate(src: &str, base: usize) -> Result<()> {
    let mut in_group = false;
    for line in Lines::new(src, base) {
        let t = line.text.trim();
        if t.starts_with('[') {
            if header_of(line.text).is_none() {
                return Err(Error::new(ErrorKind::Malformed, line.offset));
            }
            in_group = true;
            continue;
        }
        if is_skippable(line.text) {
            continue;
        }
        if !in_group {
            return Err(Error::new(ErrorKind::Malformed, line.offset));
        }
        let Some(eq) = t.find('=') else {
            return Err(Error::new(ErrorKind::Malformed, line.offset));
        };
        let key = t.get(..eq).unwrap_or("").trim_end();
        if split_key(key).is_none() {
            return Err(Error::new(ErrorKind::Malformed, line.offset));
        }
    }
    Ok(())
}

impl<'a> DesktopFile<'a> {
    /// The file text, BOM excluded.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        self.src
    }

    /// Every group, in file order.
    #[must_use]
    pub const fn groups(&self) -> Groups<'a> {
        Groups { lines: Lines::new(self.src, self.base) }
    }

    /// The first group with this exact name. §3 makes case significant, so this
    /// comparison is case-sensitive.
    #[must_use]
    pub fn group(&self, name: &str) -> Option<Group<'a>> {
        self.groups().find(|g| g.name() == name)
    }

    /// The `[Desktop Entry]` group.
    #[must_use]
    pub fn desktop_entry(&self) -> Option<Group<'a>> {
        self.group(GROUP_DESKTOP_ENTRY)
    }

    /// `Type=` (§6).
    #[must_use]
    pub fn entry_type(&self) -> Option<EntryType<'a>> {
        let v = self.desktop_entry()?.value("Type")?;
        Some(if v.eq_str("Application") {
            EntryType::Application
        } else if v.eq_str("Link") {
            EntryType::Link
        } else if v.eq_str("Directory") {
            EntryType::Directory
        } else {
            EntryType::Other(v.raw())
        })
    }

    /// `Name`, resolved for `locale` if one is given (§5).
    #[must_use]
    pub fn name(&self, locale: Option<&Locale<'_>>) -> Option<Value<'a>> {
        self.desktop_entry()?.localized("Name", locale)
    }

    /// `URL=` — present when and only when `Type=Link` (§6).
    #[must_use]
    pub fn url(&self) -> Option<Value<'a>> {
        self.desktop_entry()?.value("URL")
    }

    /// Where a `Type=Link` entry points.
    ///
    /// Returns `None` for any other `Type`, because a `Type=Application` entry
    /// points at a program, not at a document, and conflating the two is how a
    /// "open this shortcut" path turns into "run this command".
    ///
    /// The raw, still-escaped `URL` is classified. §4 permits escapes in a
    /// `string`, but a URL containing one is not a thing that occurs — the
    /// characters `\s \n \t \r` are all illegal in a URI and would be
    /// percent-encoded.
    #[must_use]
    pub fn target(&self) -> Option<ShortcutTarget<'a>> {
        if self.entry_type()? != EntryType::Link {
            return None;
        }
        Some(ShortcutTarget::classify(self.url()?.raw()))
    }

    /// `Exec=` from `[Desktop Entry]`, as structure. Never executed.
    #[must_use]
    pub fn exec(&self) -> Option<ExecCommand<'a>> {
        self.desktop_entry()?.exec()
    }

    /// The action identifiers listed in `Actions=` (§11.1).
    #[must_use]
    pub fn action_ids(&self) -> Option<ListItems<'a>> {
        Some(self.desktop_entry()?.value("Actions")?.items())
    }

    /// The `[Desktop Action <id>]` group for an identifier.
    ///
    /// §11.1: "It is not valid to have an action group for an action identifier
    /// not mentioned in the `Actions` key. Such an action group must be ignored
    /// by implementors." This does not enforce that — it looks the group up by
    /// name — so a caller building a menu should iterate [`DesktopFile::action_ids`]
    /// and call this, not iterate [`DesktopFile::groups`].
    #[must_use]
    pub fn action(&self, id: &str) -> Option<Group<'a>> {
        self.groups().find(|g| g.name().strip_prefix(GROUP_ACTION_PREFIX) == Some(id))
    }
}

/// One group and the lines belonging to it.
#[derive(Debug, Copy, Clone)]
pub struct Group<'a> {
    name: &'a str,
    body: &'a str,
    body_offset: usize,
}

impl<'a> Group<'a> {
    /// The group name, without the brackets.
    #[must_use]
    pub const fn name(&self) -> &'a str {
        self.name
    }

    /// Every entry in the group, in file order, localized ones included.
    #[must_use]
    pub const fn entries(&self) -> Entries<'a> {
        Entries { lines: Lines::new(self.body, self.body_offset) }
    }

    /// The first unpostfixed entry with this key.
    #[must_use]
    pub fn value(&self, key: &str) -> Option<Value<'a>> {
        self.entries().find(|e| e.key == key && e.locale.is_none()).map(|e| e.value)
    }

    /// The best value for `key` under `locale`, per the §5 ladder.
    ///
    /// Passing `None` returns the unpostfixed value, which is what a caller
    /// that has no locale should get — *not* an arbitrary translation.
    #[must_use]
    pub fn localized(&self, key: &str, locale: Option<&Locale<'_>>) -> Option<Value<'a>> {
        if let Some(locale) = locale {
            for candidate in locale.candidates() {
                let hit = self.entries().find(|e| {
                    e.key == key && e.locale.is_some_and(|l| candidate.matches_postfix(l))
                });
                if let Some(e) = hit {
                    return Some(e.value);
                }
            }
        }
        self.value(key)
    }

    /// A `boolean` key (§4).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] if the value is neither `true` nor `false`.
    pub fn boolean(&self, key: &str) -> Option<Result<bool>> {
        self.value(key).map(|v| v.as_bool())
    }

    /// A `numeric` key (§4).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] if the value is not a float.
    pub fn numeric(&self, key: &str) -> Option<Result<f64>> {
        self.value(key).map(|v| v.as_f64())
    }

    /// A `string(s)` key (§4), split on unescaped `;`.
    #[must_use]
    pub fn list(&self, key: &str) -> Option<ListItems<'a>> {
        Some(self.value(key)?.items())
    }

    /// This group's `Exec=`, as structure. Never executed.
    #[must_use]
    pub fn exec(&self) -> Option<ExecCommand<'a>> {
        let v = self.value("Exec")?;
        Some(ExecCommand::new(v.raw(), v.offset()))
    }
}

/// One `Key=Value` or `Key[LOCALE]=Value` line.
#[derive(Debug, Copy, Clone)]
pub struct Entry<'a> {
    /// Key name with any `[LOCALE]` postfix removed.
    pub key: &'a str,
    /// The `LOCALE` postfix, if the key had one. Still raw — compare it with
    /// [`Locale::matches_postfix`] rather than by string equality, so that
    /// `de_DE.UTF-8` and `de_DE` are recognized as the same locale.
    pub locale: Option<&'a str>,
    /// The value, still escaped.
    pub value: Value<'a>,
    /// Byte offset of the start of the line.
    pub offset: usize,
}

/// Iterator over the entries of a [`Group`].
#[derive(Debug, Copy, Clone)]
pub struct Entries<'a> {
    lines: Lines<'a>,
}

impl<'a> Iterator for Entries<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Entry<'a>> {
        for line in self.lines.by_ref() {
            if is_skippable(line.text) || header_of(line.text).is_some() {
                continue;
            }
            if let Some(entry) = entry_of(line) {
                return Some(entry);
            }
        }
        None
    }
}

fn entry_of(line: Line<'_>) -> Option<Entry<'_>> {
    let eq = line.text.find('=')?;
    let key_field = line.text.get(..eq)?;
    let (key, locale) = split_key(key_field.trim())?;
    // §3.3: "Space before and after the equals sign should be ignored; the `=`
    // sign is the actual delimiter." Only the space adjacent to `=` — a value's
    // own trailing space would have been written as `\s`.
    let raw = line.text.get(eq + 1..)?;
    let trimmed = raw.trim_start();
    let value_offset = line.offset + eq + 1 + (raw.len() - trimmed.len());
    Some(Entry {
        key,
        locale,
        value: Value::new(trimmed.trim_end(), value_offset),
        offset: line.offset,
    })
}

/// Iterator over the groups of a [`DesktopFile`].
#[derive(Debug, Copy, Clone)]
pub struct Groups<'a> {
    lines: Lines<'a>,
}

impl<'a> Iterator for Groups<'a> {
    type Item = Group<'a>;

    fn next(&mut self) -> Option<Group<'a>> {
        let name = loop {
            let line = self.lines.next()?;
            if let Some(name) = header_of(line.text) {
                break name;
            }
        };
        let body_offset = self.lines.offset();
        let from_here = self.lines.rest();

        // Walk to just before the next header, leaving the cursor on it. The
        // body is carved out by length difference between two views of the same
        // `&str`, so no index derived from the input is ever constructed.
        let body_end = loop {
            let before_next = self.lines;
            match self.lines.next() {
                None => break "",
                Some(l) if header_of(l.text).is_some() => {
                    self.lines = before_next;
                    break before_next.rest();
                }
                Some(_) => {}
            }
        };
        let body = &from_here[..from_here.len() - body_end.len()];
        Some(Group { name, body, body_offset })
    }
}

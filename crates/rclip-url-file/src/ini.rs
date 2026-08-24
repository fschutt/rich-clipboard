//! The INI-ish reader `.url` files are written against.
//!
//! `.url` is not a spec, it is "whatever `GetPrivateProfileString` accepts",
//! and this module reproduces the parts of that API's behaviour that a real
//! file depends on:
//!
//! - **Section and key names compare ASCII-case-insensitively.** Wine's
//!   `IPersistFile::Save` writes `ICONFILE=`/`ICONINDEX=` and its `Load` reads
//!   them back as `iconfile`/`iconindex`
//!   (`dlls/ieframe/intshcut.c`, `get_profile_string`), which only round-trips
//!   because the Win32 profile API folds case. A case-sensitive parser silently
//!   loses the icon on files written by Wine and by several installers.
//! - **Whitespace around `=` and around the value is stripped.**
//! - **A matched pair of double quotes around the value is stripped**, because
//!   `GetPrivateProfileString` does that and installers rely on it to keep
//!   trailing spaces.
//! - **The first occurrence of a key wins.** `GetPrivateProfileString` returns
//!   the first match; duplicates are not diagnosed here for the reason in
//!   [`crate::parse`].
//! - **`;` starts a comment**, `#` does not. The Win32 profile API only knows
//!   `;`, so a leading `#` is part of the line.

use rclip_core::{Error, ErrorKind, Result};

use crate::lines::{Line, Lines};

/// One `[Name]` section and the lines that belong to it.
#[derive(Debug, Copy, Clone)]
pub struct Section<'a> {
    name: &'a str,
    body: &'a str,
    body_offset: usize,
}

impl<'a> Section<'a> {
    /// The section name, without the brackets, exactly as written.
    #[must_use]
    pub const fn name(&self) -> &'a str {
        self.name
    }

    /// `true` if this section is `name`, comparing ASCII case-insensitively.
    #[must_use]
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    /// Every `key=value` pair in the section, in file order.
    #[must_use]
    pub const fn entries(&self) -> Entries<'a> {
        Entries { lines: Lines::new(self.body, self.body_offset) }
    }

    /// The value of the first entry whose key matches, ASCII-case-insensitively.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&'a str> {
        self.entries().find(|e| e.key.eq_ignore_ascii_case(key)).map(|e| e.value)
    }
}

/// One `key=value` line.
#[derive(Debug, Copy, Clone)]
pub struct Entry<'a> {
    /// Key name, trimmed. Case is as written; compare with
    /// [`str::eq_ignore_ascii_case`].
    pub key: &'a str,
    /// Value, trimmed and with a matched pair of surrounding quotes removed.
    pub value: &'a str,
    /// Byte offset of the start of the line this came from.
    pub offset: usize,
}

/// Iterator over the entries of one section.
#[derive(Debug, Copy, Clone)]
pub struct Entries<'a> {
    lines: Lines<'a>,
}

impl<'a> Iterator for Entries<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Entry<'a>> {
        for line in self.lines.by_ref() {
            if let Some(entry) = entry_of(line) {
                return Some(entry);
            }
        }
        None
    }
}

/// Iterator over the sections of a file.
#[derive(Debug, Copy, Clone)]
pub struct Sections<'a> {
    lines: Lines<'a>,
}

impl<'a> Sections<'a> {
    pub(crate) const fn new(src: &'a str, offset: usize) -> Self {
        Self { lines: Lines::new(src, offset) }
    }
}

impl<'a> Iterator for Sections<'a> {
    type Item = Section<'a>;

    fn next(&mut self) -> Option<Section<'a>> {
        let name = loop {
            let line = self.lines.next()?;
            if let Some(name) = header_of(line.text) {
                break name;
            }
        };
        let body_offset = self.lines.offset();
        let from_here = self.lines.rest();

        // Walk to just before the next header, leaving the cursor *on* it so
        // the next call picks it up. The body is then carved out by length
        // difference between two views of the same `&str` — no index derived
        // from the input, so there is nothing here to get wrong.
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
        Some(Section { name, body, body_offset })
    }
}

/// The section name in `[Name]`, or `None` if the line is not a header.
///
/// Trailing whitespace after `]` is tolerated: Windows' profile API ignores it
/// and real files written by installers have it.
fn header_of(text: &str) -> Option<&str> {
    let t = text.trim_end();
    let inner = t.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner)
}

/// `true` if the line is blank or a `;` comment.
fn is_skippable(text: &str) -> bool {
    let t = text.trim();
    t.is_empty() || t.starts_with(';')
}

fn entry_of(line: Line<'_>) -> Option<Entry<'_>> {
    if is_skippable(line.text) || header_of(line.text).is_some() {
        return None;
    }
    let eq = line.text.find('=')?;
    let key = line.text.get(..eq)?.trim();
    let value = unquote(line.text.get(eq + 1..)?.trim());
    Some(Entry { key, value, offset: line.offset })
}

/// Strip one matched pair of double quotes, mirroring
/// `GetPrivateProfileString`. A single unmatched quote is left alone.
fn unquote(v: &str) -> &str {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v.get(1..v.len() - 1).unwrap_or(v)
    } else {
        v
    }
}

/// Structural check run once by [`crate::parse`].
///
/// Two things are rejected, and only two, because everything else in this
/// format is optional and a `.url` on the clipboard is worth salvaging:
///
/// - a `[` line with no closing `]` — the file was truncated mid-header, and
///   silently treating it as an entry would attribute the rest of the file to
///   the previous section;
/// - a `key=value` before the first header — there is no section to file it
///   under, and Win32 would drop it on the floor.
pub(crate) fn validate(src: &str, offset: usize) -> Result<()> {
    let mut seen_header = false;
    for line in Lines::new(src, offset) {
        let t = line.text.trim_end();
        if t.starts_with('[') {
            if header_of(t).is_none() {
                return Err(Error::new(ErrorKind::Malformed, line.offset));
            }
            seen_header = true;
            continue;
        }
        if is_skippable(t) {
            continue;
        }
        if !seen_header {
            return Err(Error::new(ErrorKind::Malformed, line.offset));
        }
    }
    Ok(())
}

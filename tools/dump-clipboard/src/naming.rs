//! Turning a clipboard format identifier into a filename that survives git,
//! macOS, Windows and Linux.
//!
//! The identifiers are not filenames and were never meant to be: `text/html`
//! has a slash, `Preferred DropEffect` has a space, `UniformResourceLocatorW`
//! is fine but its neighbour `UniformResourceLocator` differs only in a
//! suffix, and macOS hands out `dyn.ah62d4rv4ge8` UTIs that are legal,
//! filename-safe and completely opaque.
//!
//! Two rules drive everything here:
//!
//! 1. **No collisions, ever.** Sanitising is lossy — `text/html` and a
//!    hypothetical `text:html` both fold to `text_html` — so a name that is
//!    already taken gets a `-2`, `-3`, … suffix rather than overwriting. The
//!    comparison is case-insensitive because APFS and NTFS are, and a corpus
//!    checked out on one of those must not lose a file.
//! 2. **The original survives.** The sidecar always records the exact native
//!    identifier, so the sanitised name is a convenience and never the record.

use std::collections::HashSet;

/// Characters kept verbatim. Everything else becomes `_`.
fn is_safe(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+')
}

/// Names Windows refuses to create a file with, in any case, with or without
/// an extension. A corpus that cannot be checked out on Windows is a corpus
/// that will not be maintained on Windows.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Leave room for a `-NN` disambiguator and the longest extension (`.json`)
/// inside the 255-byte limit every filesystem in play shares.
const MAX_STEM: usize = 96;

/// Sanitise one identifier, ignoring collisions.
fn sanitize(native: &str) -> String {
    let mut s: String = native
        .chars()
        .map(|c| if is_safe(c) { c } else { '_' })
        .collect();

    // Truncate on a char boundary; every kept char is ASCII, so byte and char
    // indices agree, but the replacement pass has already guaranteed that.
    if s.len() > MAX_STEM {
        s.truncate(MAX_STEM);
    }

    // Windows rejects trailing dots and spaces; a leading dot makes the file
    // invisible to `ls` and to a careless `git add`.
    while s.ends_with('.') {
        s.pop();
    }
    if s.starts_with('.') {
        s.insert(0, '_');
    }

    if s.is_empty() {
        return "unnamed".to_owned();
    }

    let base = s.split('.').next().unwrap_or(&s).to_ascii_lowercase();
    if RESERVED.contains(&base.as_str()) {
        s.insert(0, '_');
    }
    s
}

/// Hands out unique stems for one output directory.
#[derive(Debug, Default)]
pub struct Namer {
    used: HashSet<String>,
}

impl Namer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A unique, filesystem-safe stem for `native`.
    ///
    /// `item` prefixes the stem when the source exposed several items, so a
    /// three-file Finder copy sorts as `item-00.public.file-url`,
    /// `item-01.public.file-url`, … rather than colliding three ways.
    pub fn stem(&mut self, native: &str, item: Option<usize>) -> String {
        let mut stem = sanitize(native);
        if let Some(i) = item {
            stem = format!("item-{i:02}.{stem}");
        }

        let key = stem.to_ascii_lowercase();
        if self.used.insert(key) {
            return stem;
        }
        for n in 2u32.. {
            let candidate = format!("{stem}-{n}");
            if self.used.insert(candidate.to_ascii_lowercase()) {
                return candidate;
            }
        }
        unreachable!("u32 exhausted while disambiguating {stem}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separators_and_spaces_are_replaced() {
        assert_eq!(sanitize("text/html"), "text_html");
        assert_eq!(sanitize("Preferred DropEffect"), "Preferred_DropEffect");
        assert_eq!(
            sanitize("text/plain;charset=utf-8"),
            "text_plain_charset_utf-8"
        );
    }

    #[test]
    fn identifiers_that_are_already_safe_are_untouched() {
        for s in [
            "public.utf8-plain-text",
            "dyn.ah62d4rv4ge8",
            "CF_UNICODETEXT",
            "UniformResourceLocatorW",
        ] {
            assert_eq!(sanitize(s), s);
        }
    }

    #[test]
    fn hidden_files_and_trailing_dots_are_avoided() {
        // Prefixed rather than substituted: `_hidden` could collide with an
        // identifier that really is called that.
        assert_eq!(sanitize(".hidden"), "_.hidden");
        assert_eq!(sanitize("trailing."), "trailing");
        assert_eq!(sanitize("..."), "unnamed");
        assert_eq!(sanitize(""), "unnamed");
    }

    #[test]
    fn windows_device_names_are_escaped() {
        assert_eq!(sanitize("NUL"), "_NUL");
        assert_eq!(sanitize("com1.txt"), "_com1.txt");
        assert_eq!(sanitize("console"), "console");
    }

    #[test]
    fn long_identifiers_are_truncated_but_still_unique() {
        let a = "x".repeat(400);
        let b = format!("{a}-different-tail");
        let mut n = Namer::new();
        let first = n.stem(&a, None);
        let second = n.stem(&b, None);
        assert_eq!(first.len(), MAX_STEM);
        assert_ne!(first, second);
    }

    #[test]
    fn collisions_get_a_suffix_case_insensitively() {
        let mut n = Namer::new();
        assert_eq!(n.stem("text/html", None), "text_html");
        assert_eq!(n.stem("text:html", None), "text_html-2");
        // Case-insensitive: the taken `text_html-2` pushes this one to `-3`,
        // which is what keeps the corpus checkoutable on APFS and NTFS.
        assert_eq!(n.stem("TEXT/HTML", None), "TEXT_HTML-3");
        assert_eq!(n.stem("text|html", None), "text_html-4");
    }

    #[test]
    fn item_index_prefixes_the_stem() {
        let mut n = Namer::new();
        assert_eq!(
            n.stem("public.file-url", Some(0)),
            "item-00.public.file-url"
        );
        assert_eq!(
            n.stem("public.file-url", Some(1)),
            "item-01.public.file-url"
        );
    }
}

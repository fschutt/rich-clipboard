//! The shared "points somewhere" type for the shortcut family.
//!
//! A `text/uri-list` entry is the Linux spelling of "this points somewhere", so
//! it maps onto the same type `.url`, `.webloc` and `.desktop` (`Type=Link`)
//! use — see `plan/PLAN.md` §4.10. This is a deliberate byte-identical mirror
//! of the definition in `rclip-url-file`, which is where the family's canonical
//! copy currently lives; codec crates in this workspace do not depend on each
//! other, so the type is duplicated rather than imported.
//!
//! Keep the two in sync. Any change here belongs there too.
//
// TODO(phase-4): hoist this into `rclip-core` and have all four shortcut crates
// re-export it, deleting this mirror.

/// Where a shortcut points.
///
/// Borrowed from the parsed file, never resolved. Nothing in this workspace
/// touches the filesystem, so a [`ShortcutTarget::Path`] is a string that
/// *looks* like a path and not a path that exists.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ShortcutTarget<'a> {
    /// An absolute URI, verbatim. Still percent-encoded.
    Url(&'a str),
    /// A filesystem path, in whatever convention the source format used.
    Path(&'a str),
    /// The shortcut names a destination this crate cannot classify — a bare
    /// relative name, a shell moniker, an empty value. Handed back rather than
    /// rejected, because "I could not classify it" is information the caller
    /// may still be able to act on.
    Unresolved(&'a str),
}

impl<'a> ShortcutTarget<'a> {
    /// The underlying text, whichever variant this is.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        match self {
            Self::Url(s) | Self::Path(s) | Self::Unresolved(s) => s,
        }
    }

    /// Classify a raw destination string.
    ///
    /// The order of the tests below is the whole trick. `C:\Users\me` is a
    /// *syntactically valid* RFC 3986 URI reference with scheme `C`, so a
    /// naive "does it contain a colon" check turns every Windows path on the
    /// clipboard into a URL with a one-letter scheme. The drive-letter and
    /// UNC tests therefore run first, and only what survives them is offered
    /// to the scheme parser.
    #[must_use]
    pub fn classify(s: &'a str) -> Self {
        if s.is_empty() {
            return Self::Unresolved(s);
        }
        if looks_like_path(s) {
            return Self::Path(s);
        }
        if scheme(s).is_some() {
            return Self::Url(s);
        }
        Self::Unresolved(s)
    }
}

/// `true` for the path shapes that would otherwise be misread as URI schemes.
#[must_use]
pub fn looks_like_path(s: &str) -> bool {
    let b = s.as_bytes();
    // POSIX absolute path.
    if b[0] == b'/' {
        return true;
    }
    // UNC (`\\server\share`) and the extended-length prefix (`\\?\C:\...`).
    if b.len() >= 2 && b[0] == b'\\' && b[1] == b'\\' {
        return true;
    }
    // `X:\` or `X:/` — a DOS drive letter, not a URI scheme.
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
    {
        return true;
    }
    false
}

/// The RFC 3986 §3.1 scheme of `s`, if it has one.
///
/// `scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`. Callers must rule out
/// DOS paths first; see [`ShortcutTarget::classify`].
#[must_use]
pub fn scheme(s: &str) -> Option<&str> {
    let colon = s.find(':')?;
    let head = s.get(..colon)?;
    let mut chars = head.chars();
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    if chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        Some(head)
    } else {
        None
    }
}

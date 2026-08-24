//! The shortcut family, unified.
//!
//! Windows `.url`, macOS `.webloc` / `.inetloc`, macOS `BookmarkData` and
//! freedesktop `.desktop` (`Type=Link`) are four spellings of one idea: a file
//! that points somewhere. `plan/PLAN.md` §4.10 wants them behind a single type,
//! and each of the four codec crates carries a byte-identical borrowed
//! `ShortcutTarget` with a `// TODO(phase-4): hoist this into rclip-core`
//! against it — the crates cannot share the type because codec crates in this
//! workspace do not depend on each other.
//!
//! [`LinkTarget`] is the owned, consumer-facing version of that type, and this
//! module is where the four parsers meet. When the phase-4 hoist happens, this
//! becomes a conversion rather than a redefinition.
//!
//! # Nothing here resolves anything
//!
//! A [`LinkTarget::Path`] is a string that *looks* like a path. No filesystem
//! is consulted, no `.desktop` `Exec=` is expanded, no URL is opened. The
//! `.desktop` case is called out by name in `plan/CONVENTIONS.md` rule 6, and
//! for good reason: a `.desktop` file that arrived over the clipboard was
//! written by another process and describes something to run.

use alloc::string::String;

/// Where a shortcut points.
///
/// The owned mirror of the borrowed `ShortcutTarget` that `rclip-url-file`,
/// `rclip-uri-list` and `rclip-desktop-entry` each define.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LinkTarget {
    /// An absolute URI, verbatim. Still percent-encoded.
    Url(String),
    /// A filesystem path, in whatever convention the source format used.
    Path(String),
    /// A destination that could not be classified — a bare relative name, a
    /// shell moniker, an empty value. Handed back rather than rejected: "I
    /// could not classify it" is information the caller may still act on.
    Unresolved(String),
}

impl LinkTarget {
    /// The underlying text, whichever variant this is.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Url(s) | Self::Path(s) | Self::Unresolved(s) => s,
        }
    }

    /// Classify a raw destination string.
    ///
    /// The order of the tests is the whole trick, and it is the reason this is
    /// not a `contains(':')`: `C:\Users\me` is a syntactically valid RFC 3986
    /// URI reference with the scheme `C`, so a naive check turns every Windows
    /// path on the clipboard into a URL. The path shapes are ruled out first
    /// and only what survives is offered to the scheme parser.
    #[must_use]
    pub fn classify(s: &str) -> Self {
        let owned = String::from(s);
        if s.is_empty() {
            return Self::Unresolved(owned);
        }
        if looks_like_path(s) {
            return Self::Path(owned);
        }
        if scheme(s).is_some() {
            return Self::Url(owned);
        }
        Self::Unresolved(owned)
    }
}

/// `true` for the path shapes that would otherwise be misread as URI schemes.
#[must_use]
pub(crate) fn looks_like_path(s: &str) -> bool {
    let b = s.as_bytes();
    match b {
        // POSIX absolute path.
        [b'/', ..] => true,
        // UNC (`\\server\share`) and the extended-length prefix (`\\?\C:\…`).
        [b'\\', b'\\', ..] => true,
        // `X:\` or `X:/` — a DOS drive letter, not a URI scheme.
        [d, b':', b'\\' | b'/', ..] => d.is_ascii_alphabetic(),
        _ => false,
    }
}

/// The RFC 3986 §3.1 scheme of `s`, if it has one.
///
/// `scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`. Callers must rule out
/// DOS paths first; see [`LinkTarget::classify`].
#[must_use]
pub(crate) fn scheme(s: &str) -> Option<&str> {
    let colon = s.find(':')?;
    let head = s.get(..colon)?;
    let mut chars = head.chars();
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    chars
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        .then_some(head)
}

/// Something that points somewhere, with the title that came with it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link {
    /// The destination.
    pub target: LinkTarget,
    /// A human-readable title, when the format carried one: `public.url-name`,
    /// a `.inetloc`'s `URLName`, a `.desktop`'s `Name=`, a bookmark's display
    /// name.
    pub title: Option<String>,
}

impl Link {
    /// A link to `url`, with no title.
    #[must_use]
    pub fn to_url(url: impl AsRef<str>) -> Self {
        Self {
            target: LinkTarget::classify(url.as_ref()),
            title: None,
        }
    }

    /// Attach a title.
    #[must_use]
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

// ---------------------------------------------------------------------------
// The four file formats
// ---------------------------------------------------------------------------

/// Which shortcut file format a blob is, as far as sniffing can tell.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ShortcutFormat {
    /// Windows `.url`.
    UrlFile,
    /// macOS `.webloc` / `.inetloc`.
    Webloc,
    /// macOS `BookmarkData`.
    Bookmark,
    /// freedesktop `.desktop`.
    DesktopEntry,
}

/// Sniff which of the four a blob is.
///
/// Magic first — `book`/`alis` and `bplist00` are unambiguous — then the two
/// INI-shaped text formats, which are told apart by their group header. A
/// `.webloc` in XML form is a plist, so it is checked before the text formats
/// even though it is text.
pub(crate) fn sniff(bytes: &[u8]) -> Option<ShortcutFormat> {
    if bytes.starts_with(b"book") || bytes.starts_with(b"alis") {
        return Some(ShortcutFormat::Bookmark);
    }
    if bytes.starts_with(b"bplist00") {
        return Some(ShortcutFormat::Webloc);
    }
    // Skip a UTF-8 BOM and leading whitespace before looking at the first
    // meaningful character; every one of these formats occurs with a BOM,
    // because that is what a Windows text editor writes by default.
    let text = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let text = text
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map_or(&[][..], |i| &text[i..]);
    if text.starts_with(b"<") {
        return Some(ShortcutFormat::Webloc);
    }
    // Both remaining formats open with a group header; only the name differs.
    if text.starts_with(b"[Desktop Entry]") {
        return Some(ShortcutFormat::DesktopEntry);
    }
    if text.starts_with(b"[") {
        return Some(ShortcutFormat::UrlFile);
    }
    None
}

#[cfg(feature = "alloc")]
mod parsers {
    use alloc::string::String;

    use crate::error::{Error, Result};

    #[allow(unused_imports)]
    use super::{sniff, Link, LinkTarget, ShortcutFormat};

    impl Link {
        /// Parse a shortcut file of any of the four supported kinds.
        ///
        /// Sniffs the format, then dispatches. Use this for a file that was
        /// dropped on the application: the extension is a hint the sender chose
        /// and the bytes are not.
        ///
        /// # Errors
        ///
        /// [`Error::Unsupported`] if the bytes are none of the four, or if the
        /// one they are needs a feature this build does not have, and
        /// [`Error::Codec`] if the matching parser rejected them.
        // With every shortcut feature on, the four arms below are exhaustive
        // and the `FeatureDisabled` arm is dead. That arm exists precisely for
        // the builds where they are not, so its deadness is the good case.
        #[allow(unreachable_patterns)]
        pub fn from_file(bytes: &[u8]) -> Result<Self> {
            match sniff(bytes) {
                #[cfg(feature = "url-file")]
                Some(ShortcutFormat::UrlFile) => Self::from_url_file(bytes),
                #[cfg(feature = "webloc")]
                Some(ShortcutFormat::Webloc) => Self::from_webloc(bytes),
                #[cfg(feature = "bookmark")]
                Some(ShortcutFormat::Bookmark) => Self::from_bookmark(bytes),
                #[cfg(feature = "desktop-entry")]
                Some(ShortcutFormat::DesktopEntry) => Self::from_desktop_entry(bytes),
                Some(format) => Err(Error::FeatureDisabled {
                    flavor: match format {
                        ShortcutFormat::UrlFile => ".url",
                        ShortcutFormat::Webloc => ".webloc",
                        ShortcutFormat::Bookmark => "BookmarkData",
                        ShortcutFormat::DesktopEntry => ".desktop",
                    },
                    feature: match format {
                        ShortcutFormat::UrlFile => "url-file",
                        ShortcutFormat::Webloc => "webloc",
                        ShortcutFormat::Bookmark => "bookmark",
                        ShortcutFormat::DesktopEntry => "desktop-entry",
                    },
                }),
                None => Err(Error::Unsupported {
                    native: String::from("(unrecognised shortcut file)"),
                }),
            }
        }

        /// Parse a Windows `.url` `[InternetShortcut]` file.
        ///
        /// # What is lost
        ///
        /// `IconFile`, `IconIndex`, `HotKey`, `ShowCommand`, `Modified`,
        /// `WorkingDirectory` and `IDList`. [`Link`] models a destination and a
        /// title; reach for `rclip_url_file::UrlFile` directly for the rest.
        ///
        /// # Errors
        ///
        /// [`Error::Codec`] if the file is not UTF-8 or its INI structure is
        /// malformed, and if it has no `URL=` — the one key the format
        /// requires.
        #[cfg(feature = "url-file")]
        pub fn from_url_file(bytes: &[u8]) -> Result<Self> {
            let file = rclip_url_file::parse(bytes).map_err(|e| Error::codec(".url", e))?;
            let url = file.require_url().map_err(|e| Error::codec(".url", e))?;
            Ok(Self {
                target: LinkTarget::classify(url),
                title: None,
            })
        }

        /// Parse a macOS `.webloc` or `.inetloc`, in either the XML or the
        /// `bplist00` encoding.
        ///
        /// # Errors
        ///
        /// [`Error::Codec`] if the plist is malformed or has no `URL` key.
        #[cfg(feature = "webloc")]
        pub fn from_webloc(bytes: &[u8]) -> Result<Self> {
            let loc = rclip_webloc::Webloc::parse(bytes).map_err(|e| Error::codec(".webloc", e))?;
            Ok(Self {
                target: LinkTarget::classify(&loc.url().to_string_lossy()),
                title: loc.url_name().map(|n| n.to_string_lossy()),
            })
        }

        /// Parse a macOS `BookmarkData` blob — a Finder alias, or what
        /// `NSURL.bookmarkData` returns.
        ///
        /// # What is lost
        ///
        /// Everything a bookmark is *for*. A bookmark keeps the target's
        /// catalog node ID and its volume's UUID so macOS can resolve it after
        /// the file has been moved; [`Link`] keeps the `file://` URL, which is
        /// exactly the part that breaks when that happens. Reach for
        /// `rclip_bookmark::Bookmark` when the resolution matters.
        ///
        /// # Errors
        ///
        /// [`Error::Codec`] for a bad header or an offset that does not
        /// resolve, and [`Error::Unsupported`] if the bookmark names no target
        /// at all.
        #[cfg(feature = "bookmark")]
        pub fn from_bookmark(bytes: &[u8]) -> Result<Self> {
            let bm = rclip_bookmark::Bookmark::parse(bytes)
                .map_err(|e| Error::codec("BookmarkData", e))?;
            let url = bm
                .target_url()
                .map_err(|e| Error::codec("BookmarkData", e))?;
            let title = bm
                .display_name()
                .map_err(|e| Error::codec("BookmarkData", e))?
                .or(bm
                    .target_filename()
                    .map_err(|e| Error::codec("BookmarkData", e))?)
                .map(String::from);
            match url {
                Some(url) => Ok(Self {
                    target: LinkTarget::classify(url),
                    title,
                }),
                // A bookmark with no `0x1003` target URL is well-formed and
                // useless to a `Link`. Reconstructing one from the `0x1004`
                // path components would be inventing a path the writer did not
                // state.
                None => Err(Error::Unsupported {
                    native: String::from("BookmarkData without a target URL"),
                }),
            }
        }

        /// Parse a freedesktop `.desktop` entry.
        ///
        /// Only `Type=Link` yields a destination. A `Type=Application` entry
        /// describes a *program to run*, which is not a place, and this crate
        /// will not turn one into a `Link` — parse it with
        /// `rclip_desktop_entry` and decide deliberately.
        ///
        /// # What is lost
        ///
        /// Localized names other than the unlocalized one, `Icon`, `Comment`,
        /// actions, and everything about `Type=Application`.
        ///
        /// # Errors
        ///
        /// [`Error::Codec`] if the file is not UTF-8 or its group structure is
        /// malformed, and [`Error::Unsupported`] if it is not a `Type=Link`
        /// entry or has no `URL=`.
        #[cfg(feature = "desktop-entry")]
        pub fn from_desktop_entry(bytes: &[u8]) -> Result<Self> {
            use rclip_desktop_entry::EntryType;

            let file =
                rclip_desktop_entry::parse(bytes).map_err(|e| Error::codec(".desktop", e))?;
            if file.entry_type() != Some(EntryType::Link) {
                return Err(Error::Unsupported {
                    native: String::from(".desktop entry that is not Type=Link"),
                });
            }
            let Some(url) = file.url() else {
                return Err(Error::Unsupported {
                    native: String::from("Type=Link .desktop entry without a URL"),
                });
            };
            Ok(Self {
                target: LinkTarget::classify(&url.to_unescaped_lossy()),
                title: file.name(None).map(|n| n.to_unescaped_lossy()),
            })
        }
    }
}

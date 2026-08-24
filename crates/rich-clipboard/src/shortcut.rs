//! The shortcut family, unified.
//!
//! Windows `.url`, macOS `.webloc` / `.inetloc`, macOS `BookmarkData` and
//! freedesktop `.desktop` (`Type=Link`) are four spellings of one idea: a file
//! that points somewhere, and `plan/PLAN.md` §4.10 puts them behind a single
//! type. That type is [`rclip_core::ShortcutTarget`], which every codec crate
//! in the family re-exports; until Phase 4 hoisted it, four of them carried a
//! byte-identical copy and a consumer holding two of them held two
//! incompatible types for one concept.
//!
//! [`LinkTarget`] is the **owned** counterpart, for the same reason every other
//! type in this crate is owned: a `ShortcutTarget<'a>` borrows from the blob it
//! was parsed out of, and a [`Link`] outlives that blob. It is a conversion of
//! the shared type and not a second definition of it — [`LinkTarget::classify`]
//! *is* [`ShortcutTarget::classify`] with the result copied out, the two
//! agree variant for variant, and [`LinkTarget::as_target`] and the
//! [`From`] impl move between them.
//!
//! # Nothing here resolves anything
//!
//! A [`LinkTarget::Path`] is a string that *looks* like a path. No filesystem
//! is consulted, no `.desktop` `Exec=` is expanded, no URL is opened. The
//! `.desktop` case is called out by name in `plan/CONVENTIONS.md` rule 6, and
//! for good reason: a `.desktop` file that arrived over the clipboard was
//! written by another process and describes something to run.

use alloc::string::String;

use rclip_core::ShortcutTarget;

/// Where a shortcut points, owned.
///
/// The owned counterpart of [`ShortcutTarget`], which the codec crates return
/// borrowed from the blob they parsed. Variant for variant the same type; the
/// classification is not reimplemented here, it is delegated.
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

    /// Borrow this as the shared [`ShortcutTarget`].
    ///
    /// Free: same variant, same characters. This is the direction that lets a
    /// consumer holding a [`Link`] hand it to anything in the family that
    /// speaks the borrowed type.
    #[must_use]
    pub fn as_target(&self) -> ShortcutTarget<'_> {
        match self {
            Self::Url(s) => ShortcutTarget::Url(s),
            Self::Path(s) => ShortcutTarget::Path(s),
            Self::Unresolved(s) => ShortcutTarget::Unresolved(s),
        }
    }

    /// Classify a raw destination string.
    ///
    /// Delegates to [`ShortcutTarget::classify`] and copies the result out, so
    /// that the owned type and the borrowed one cannot drift. The order of the
    /// tests there is the whole trick, and it is the reason this is not a
    /// `contains(':')`: `C:\Users\me` is a syntactically valid RFC 3986 URI
    /// reference with the scheme `C`, so a naive check turns every Windows path
    /// on the clipboard into a URL.
    #[must_use]
    pub fn classify(s: &str) -> Self {
        ShortcutTarget::classify(s).into()
    }
}

impl From<ShortcutTarget<'_>> for LinkTarget {
    /// Take ownership of a borrowed target.
    ///
    /// Total in both directions with [`LinkTarget::as_target`]: nothing is
    /// reclassified on the way through, so a `Path` that a codec crate decided
    /// on stays a `Path` even if this crate's rules were ever to differ.
    fn from(t: ShortcutTarget<'_>) -> Self {
        match t {
            ShortcutTarget::Url(s) => Self::Url(String::from(s)),
            ShortcutTarget::Path(s) => Self::Path(String::from(s)),
            ShortcutTarget::Unresolved(s) => Self::Unresolved(String::from(s)),
            // `ShortcutTarget` is `#[non_exhaustive]`. A variant added later
            // means a destination shape this crate has no owned spelling for,
            // and the text is still the honest answer to "where does it point".
            other => Self::Unresolved(String::from(other.as_str())),
        }
    }
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

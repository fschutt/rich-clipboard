//! [`RichItem`] — what a clipboard item is, once it has been decoded.

use alloc::string::String;
use alloc::vec::Vec;

use crate::fanout::ItemKind;
use crate::rich_text::RichText;
use crate::shortcut::Link;

/// A decoded clipboard item.
///
/// One item, not one flavor: a source that offered HTML, RTF and plain text
/// offered three encodings of *one* thing, and this is that thing.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum RichItem {
    /// Unstyled UTF-8 text.
    Text(String),
    /// Styled text, decoded into the hub representation.
    RichText(RichText),
    /// An HTML fragment, still markup.
    ///
    /// Not a [`RichText`], because turning markup into styled runs needs an
    /// HTML tokenizer and there is not one in this workspace yet — see the
    /// [`rich_text`](crate::rich_text) module docs. The markup is handed over
    /// intact rather than tag-stripped into a plausible-looking lie.
    Html(HtmlFragment),
    /// A raster image.
    Image(Image),
    /// Files that exist on disk (or on a remote volume), plus whether the
    /// source cut or copied them.
    Files(FileList),
    /// Descriptors for files that do not exist yet.
    ///
    /// The bytes arrive separately, one `CFSTR_FILECONTENTS` stream per
    /// descriptor keyed by index — that is transport, so it is not here.
    PromisedFiles(Vec<PromisedFile>),
    /// Something that points somewhere: a URL flavor, or a `.url` / `.webloc` /
    /// `.desktop` / bookmark file that was handed to
    /// [`Link::from_file`](crate::Link::from_file).
    Link(Link),
    /// A parsed `.lnk`.
    Shortcut(Shortcut),
    /// Windows shell namespace objects, as display names.
    ShellItems(ShellItems),
    /// A flavor this build could not decode, carried through verbatim.
    ///
    /// Not a failure. A clipboard bridge, an inspector, or an application with
    /// its own handling for a private format all want the bytes, and
    /// [`encode`](crate::encode) republishes this under the identifier it
    /// arrived with.
    Unknown {
        /// The platform-native identifier the item arrived under.
        native: String,
        /// The undecoded bytes.
        bytes: Vec<u8>,
    },
}

impl RichItem {
    /// The key into the write-side table.
    #[must_use]
    pub const fn kind(&self) -> ItemKind {
        match self {
            Self::Text(_) => ItemKind::Text,
            Self::RichText(_) => ItemKind::RichText,
            Self::Html(_) => ItemKind::Html,
            Self::Image(_) => ItemKind::Image,
            Self::Files(_) => ItemKind::Files,
            Self::PromisedFiles(_) => ItemKind::PromisedFiles,
            Self::Link(_) => ItemKind::Link,
            Self::Shortcut(_) => ItemKind::Shortcut,
            Self::ShellItems(_) => ItemKind::ShellItems,
            Self::Unknown { .. } => ItemKind::Unknown,
        }
    }

    /// The best plain-text rendering, for the `CF_UNICODETEXT` companion every
    /// fan-out wants.
    ///
    /// `None` where there is no honest answer: an image has no text, and an
    /// HTML fragment has none either unless the caller supplied one, because
    /// stripping tags without a parser produces something that *looks* like the
    /// text and is not.
    #[must_use]
    pub fn plain_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::RichText(t) => Some(t.as_str()),
            Self::Html(h) => h.plain.as_deref(),
            Self::Link(link) => Some(link.target.as_str()),
            Self::Image(_)
            | Self::Files(_)
            | Self::PromisedFiles(_)
            | Self::Shortcut(_)
            | Self::ShellItems(_)
            | Self::Unknown { .. } => None,
        }
    }
}

/// An HTML fragment and what came with it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HtmlFragment {
    /// The markup, UTF-8. For `CF_HTML` this is the *fragment*, not the
    /// surrounding context document.
    pub markup: String,
    /// The surrounding document, when `CF_HTML` carried one. `None` for a
    /// fragment-only blob (`StartHTML:-1`) and for every non-Windows HTML
    /// flavor, which are bare markup with nothing around them.
    pub context: Option<String>,
    /// `SourceURL`, the only thing that makes a relative link in the fragment
    /// resolvable.
    pub source_url: Option<String>,
    /// A plain-text rendering, if one was available.
    ///
    /// Filled in by [`decode_payload`](crate::decode_payload) from a sibling
    /// plain-text flavor, and settable by a caller that is about to publish.
    /// Never derived from `markup`: see [`RichItem::plain_text`].
    pub plain: Option<String>,
}

/// A raster image.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Image {
    /// Decoded pixels. Only `CF_DIB` and `CF_DIBV5` arrive this way — they are
    /// the two raster formats no general image library serves, which is why
    /// this workspace owns them.
    Rgba(RgbaImage),
    /// An encoded image, undecoded on purpose.
    ///
    /// `plan/PLAN.md` §4.4 scopes PNG, JPEG, GIF and TIFF out of this
    /// workspace: they have good decoders already, and making one a hard
    /// dependency of the clipboard layer would be a poor trade for a consumer
    /// that already has its own.
    Encoded {
        /// Which format the bytes are in.
        format: ImageFormat,
        /// The bytes.
        bytes: Vec<u8>,
    },
}

/// An encoded image format this crate deliberately does not decode.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPEG / JFIF.
    Jpeg,
    /// GIF.
    Gif,
    /// TIFF — the native macOS pasteboard raster.
    Tiff,
}

/// 8-bit RGBA pixels, top row first, no row padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes of `R, G, B, A`.
    pub pixels: Vec<u8>,
}

/// What the source did to the files it put on the clipboard.
///
/// Every platform spells this differently and two of them do not spell it at
/// all unless you ask: without `Preferred DropEffect` on Windows, or
/// `x-special/gnome-copied-files` / `application/x-kde-cutselection` on Linux,
/// a paste of files always reads as a copy and the user's cut silently becomes
/// one.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
pub enum TransferAction {
    /// Leave the originals. `DROPEFFECT_COPY`, GNOME's `copy` verb.
    #[default]
    Copy,
    /// Remove the originals after a successful paste. `DROPEFFECT_MOVE`, and
    /// what GNOME and KDE call "cut".
    Move,
    /// Create a reference rather than a copy. `DROPEFFECT_LINK`; no Linux
    /// convention carries it, and the Finder has no notion of it either.
    Link,
}

/// One entry of a [`FileList`].
///
/// A file list is not always a list of local paths. GNOME will happily copy
/// something out of an `sftp://` or `smb://` mount, and flattening that to a
/// path would name a file that is not there.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FileEntry {
    /// A local filesystem path, in the source platform's convention.
    ///
    /// From a `file://` URI this is the percent-*decoded* path. It is a string
    /// that names a path, not a path that exists: nothing in this workspace
    /// touches a filesystem.
    Path(String),
    /// A URI that is not a local file, verbatim and still percent-encoded.
    Uri(String),
}

impl FileEntry {
    /// The underlying string, whichever variant this is.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Path(s) | Self::Uri(s) => s,
        }
    }

    /// The path, for a local file.
    #[must_use]
    pub fn as_path(&self) -> Option<&str> {
        match self {
            Self::Path(s) => Some(s),
            Self::Uri(_) => None,
        }
    }
}

/// Files that exist, and what the source meant to happen to them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileList {
    /// The files, in the order the source listed them.
    pub entries: Vec<FileEntry>,
    /// Cut or copy. Defaults to [`TransferAction::Copy`], which is the safe
    /// reading — guessing "move" would delete a user's files.
    pub action: TransferAction,
}

impl FileList {
    /// A copy of these paths.
    #[must_use]
    pub fn of_paths<I, S>(paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            entries: paths
                .into_iter()
                .map(|p| FileEntry::Path(p.into()))
                .collect(),
            action: TransferAction::Copy,
        }
    }

    /// `true` if there are no files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// A file that does not exist yet.
///
/// This is how Outlook drags an attachment that lives in a database, and how an
/// application offers "drag this generated PDF into Explorer" without writing a
/// temp file first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromisedFile {
    /// The name the file would have. Not a path — a `FILEDESCRIPTORW`
    /// `cFileName` may name a subdirectory, but it never names an absolute
    /// location.
    pub name: String,
    /// Size in bytes, when the descriptor stated one.
    ///
    /// `None` means the `FD_FILESIZE` flag was clear, which is different from
    /// zero: a clear flag and a zero field look identical on the wire and only
    /// one of them means "empty file".
    pub size: Option<u64>,
    /// `FILE_ATTRIBUTE_*`, when stated. Passed through unvalidated.
    pub attributes: Option<u32>,
    /// Last write time as a raw `FILETIME` — 100ns ticks since 1601-01-01 UTC —
    /// when stated.
    ///
    /// Raw rather than a date type so this crate takes no date-library
    /// dependency; `rclip-shell-link`'s `FileTime` makes the same call.
    pub last_write_filetime: Option<u64>,
    /// `true` if the descriptor is a directory.
    pub is_directory: bool,
}

/// Windows shell namespace objects, reduced to what a consumer can act on.
///
/// A PIDL names a thing in the shell namespace, and most of those things are
/// not files — the Recycle Bin, a camera, a mail message, the inside of a zip.
/// Resolving one needs a live shell, which is the operation with the CVE
/// history, so this crate stops at the display name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellItems {
    /// One display *label* per selected object, best effort.
    ///
    /// A label, never a path to open. Items whose class nobody has
    /// reverse-engineered contribute nothing rather than a placeholder — a
    /// breadcrumb with a gap is more honest than one with a fabricated segment.
    pub display_paths: Vec<String>,
    /// The parent folder's display label, when it had one.
    pub parent: Option<String>,
}

/// A parsed `.lnk`.
///
/// Every field is a string chosen by whoever made the link. Nothing here has
/// been resolved, canonicalised, or checked against a policy: expanding
/// [`Shortcut::target_path`], running [`Shortcut::arguments`], or loading
/// [`Shortcut::icon_location`] is the caller's decision and the caller's risk.
/// A shell link is untrusted input that describes something to execute, and
/// keeping the parse and the act apart is the entire defence — CVE-2010-2568
/// and CVE-2017-8464 were both bugs in the acting, not the parsing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shortcut {
    /// The drive-letter or UNC path from `LinkInfo`, if the link has one.
    pub target_path: Option<String>,
    /// The display path built from the target `ITEMIDLIST`. A label.
    pub display_path: Option<String>,
    /// `NAME_STRING` — the description shown to the user.
    pub name: Option<String>,
    /// `RELATIVE_PATH` — where the target is relative to the `.lnk` itself.
    pub relative_path: Option<String>,
    /// `WORKING_DIR`.
    pub working_dir: Option<String>,
    /// `COMMAND_LINE_ARGUMENTS`. Attacker-chosen text.
    pub arguments: Option<String>,
    /// `ICON_LOCATION`. An attacker-chosen path; loading it at display time is
    /// precisely the CVE-2010-2568 pattern.
    pub icon_location: Option<String>,
    /// The `%windir%`-style path from an `EnvironmentVariableDataBlock`, if
    /// present. The closest a `.lnk` gets to a target that means anything off
    /// the machine that wrote it.
    pub environment_path: Option<String>,
}

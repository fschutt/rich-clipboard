//! The undocumented conventions that carry **cut vs copy** on Linux.
//!
//! RFC 2483 has no notion of a transfer verb, so without one of these every
//! paste of files reads as a copy and "cut" silently becomes "duplicate". None
//! of them is specified anywhere; the grammars below were read out of the
//! implementations that define them, cited per item.
//!
//! # `x-special/gnome-copied-files`
//!
//! ```text
//! payload = verb *( LF uri )
//! verb    = "copy" / "cut"      ; lowercase ASCII, exact
//! ```
//!
//! **LF only, and no trailing newline.** From Nautilus's writer,
//! `src/nautilus-clipboard.c`:
//!
//! ```c
//! uris = g_string_new (clip->cut ? "cut" : "copy");
//! for (l = clip->files; l != NULL; l = l->next) {
//!     uri = nautilus_file_get_uri (l->data);
//!     g_string_append_c (uris, '\n');
//!     g_string_append (uris, uri);
//! }
//! ```
//!
//! The same file registers the type with GDK and calls it undocumented in a
//! comment. Thunar (`thunar-clipboard-manager.c`) and Nemo
//! (`nemo-clipboard-monitor.c`) write the identical shape.
//!
//! Getting this exactly right matters on the **write** side. Since Nautilus 44
//! (commit `ee5a3586`, "clipboard: Make Nautilus Clipboard more resilient") the
//! reader splits on `\n` and rejects the whole payload if any line is empty or
//! fails `g_uri_is_valid`. So a trailing newline, a CRLF, or an unencoded space
//! is not degraded — it is a hard failure, and the paste does nothing.
//! [`crate::emit`] therefore writes it byte-exactly.
//!
//! Reading is the other way round: [`parse_copied_files`] accepts a trailing
//! newline and CRLF, matching Thunar's reader, which case-insensitively strips
//! `copy\n`/`cut\n` and hands the rest to `g_uri_list_extract_uris`.
//!
//! # `x-special/mate-copied-files`
//!
//! MATE's Caja (`libcaja-private/caja-clipboard-monitor.c`) under a different
//! name, byte-identical grammar. Parsed by the same function.
//!
//! # `x-special/nautilus-clipboard`
//!
//! Not a MIME type — the widespread description of it as one is wrong. Between
//! Nautilus 3.30 and 3.38 the file list was published on the **plain-text**
//! target and self-identified with a magic first line:
//!
//! ```text
//! x-special/nautilus-clipboard\ncopy\nfile:///a\nfile:///b\n
//! ```
//!
//! (note the trailing LF, which the modern format does not have). Removed in
//! Nautilus 40 by commit `2045f662`, "Revert 'clipboard: Use text based
//! clipboard only'". [`parse_nautilus_text_clipboard`] reads it so that a
//! payload from a pre-40 desktop, or from one of the third-party apps that
//! did intern an atom by that name, is not misread as plain text. Nothing
//! emits it: see [`crate::emit::RECOMMENDED`].
//!
//! # `application/x-kde-cutselection`
//!
//! One ASCII byte. `KIO::setClipboardDataCut` in KIO's
//! `src/widgets/paste.cpp`:
//!
//! ```cpp
//! const QByteArray cutSelectionData = cut ? "1" : "0";
//! mimeData->setData(QStringLiteral("application/x-kde-cutselection"), cutSelectionData);
//! ...
//! return (!a.isEmpty() && a.at(0) == '1');
//! ```
//!
//! Two things the folklore gets wrong: `"0"` *is* written, for copy — this is
//! not a cut-only marker — and the reader inspects **only byte 0**, so `"1\n"`
//! reads as cut and anything else reads as copy. [`parse_kde_cut_selection`]
//! reproduces that exactly, and is infallible for the same reason KIO's is.
//!
//! The URIs are not in this payload; it is a sidecar for a separate
//! `text/uri-list`.
//!
//! # They do not interoperate
//!
//! KDE neither writes nor reads `x-special/gnome-copied-files` (checked across
//! KIO, KCoreAddons' `KUrlMimeData`, Dolphin and Plasma), and GNOME does not
//! read `application/x-kde-cutselection`. Chromium implements neither and has
//! no cut/copy distinction for Linux files at all. A source that wants its cut
//! to survive has to offer every one of them — which is what
//! [`crate::emit::RECOMMENDED`] lists.

use rclip_core::{Error, ErrorKind, Result};

use crate::{Uris, UriList};

/// `text/uri-list` (RFC 2483 §5).
pub const MIME_URI_LIST: &str = "text/uri-list";

/// `x-special/gnome-copied-files` — GNOME, Xfce, Cinnamon, COSMIC.
pub const MIME_GNOME_COPIED_FILES: &str = "x-special/gnome-copied-files";

/// `x-special/mate-copied-files` — MATE, identical grammar.
pub const MIME_MATE_COPIED_FILES: &str = "x-special/mate-copied-files";

/// `application/x-kde-cutselection` — KDE's one-byte sidecar flag.
pub const MIME_KDE_CUT_SELECTION: &str = "application/x-kde-cutselection";

/// The magic first line of the legacy Nautilus 3.30–3.38 `text/plain` payload.
/// Not a MIME type; see the module docs.
pub const NAUTILUS_TEXT_MAGIC: &str = "x-special/nautilus-clipboard";

/// What the source did to the files.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
pub enum FileAction {
    /// Paste leaves the originals in place.
    #[default]
    Copy,
    /// Paste moves the originals. The whole reason these conventions exist.
    Cut,
}

impl FileAction {
    /// The verb as `x-special/gnome-copied-files` spells it.
    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Cut => "cut",
        }
    }

    /// The single byte `application/x-kde-cutselection` carries.
    ///
    /// `b"0"` for copy is deliberate: KIO writes it, and a reader that treats
    /// "payload present" as "cut" would turn every KDE copy into a move.
    #[must_use]
    pub const fn kde_payload(self) -> &'static [u8] {
        match self {
            Self::Copy => b"0",
            Self::Cut => b"1",
        }
    }
}

/// A verb plus a list of URIs.
#[derive(Debug, Copy, Clone)]
pub struct CopiedFiles<'a> {
    action: FileAction,
    uris: UriList<'a>,
}

impl<'a> CopiedFiles<'a> {
    /// Cut or copy.
    #[must_use]
    pub const fn action(&self) -> FileAction {
        self.action
    }

    /// The URIs, as a [`UriList`].
    #[must_use]
    pub const fn uri_list(&self) -> UriList<'a> {
        self.uris
    }

    /// The URIs, skipping comment lines.
    #[must_use]
    pub const fn uris(&self) -> Uris<'a> {
        self.uris.uris()
    }
}

/// Parse an `x-special/gnome-copied-files` or `x-special/mate-copied-files`
/// payload.
///
/// Lenient in the ways Thunar's reader is: the verb is matched
/// case-insensitively, and a trailing newline or CRLF terminators are accepted.
/// See the module docs for why the *writer* must not be lenient in return.
///
/// # Errors
///
/// - [`ErrorKind::InvalidUtf8`] if the payload is not UTF-8.
/// - [`ErrorKind::BadMagic`] if the first line is neither `copy` nor `cut`.
///   Not "assume copy": a payload whose first line is something else is not
///   this format, and treating an unknown first line as a URI would put a
///   stray entry at the head of the file list.
pub fn parse_copied_files(bytes: &[u8]) -> Result<CopiedFiles<'_>> {
    let list = crate::parse(bytes)?;
    let mut lines = list.raw_lines();
    let Some(first) = lines.next() else {
        return Err(Error::new(ErrorKind::UnexpectedEof, 0));
    };
    let action = action_of(first.text)
        .ok_or_else(|| Error::new(ErrorKind::BadMagic, first.offset))?;
    Ok(CopiedFiles { action, uris: list.tail_of(&lines) })
}

/// Parse the legacy Nautilus 3.30–3.38 `text/plain` payload.
///
/// Recognized by its magic first line; see [`NAUTILUS_TEXT_MAGIC`].
///
/// # Errors
///
/// - [`ErrorKind::InvalidUtf8`] if the payload is not UTF-8.
/// - [`ErrorKind::BadMagic`] if the first line is not the magic string or the
///   second is not a verb.
pub fn parse_nautilus_text_clipboard(bytes: &[u8]) -> Result<CopiedFiles<'_>> {
    let list = crate::parse(bytes)?;
    let mut lines = list.raw_lines();
    let Some(magic) = lines.next() else {
        return Err(Error::new(ErrorKind::UnexpectedEof, 0));
    };
    if magic.text.trim() != NAUTILUS_TEXT_MAGIC {
        return Err(Error::new(ErrorKind::BadMagic, magic.offset));
    }
    let Some(verb) = lines.next() else {
        return Err(Error::new(ErrorKind::UnexpectedEof, magic.offset + magic.text.len()));
    };
    let action =
        action_of(verb.text).ok_or_else(|| Error::new(ErrorKind::BadMagic, verb.offset))?;
    Ok(CopiedFiles { action, uris: list.tail_of(&lines) })
}

/// `true` if a payload looks like the legacy Nautilus text clipboard.
///
/// Cheap enough to call before deciding whether a `text/plain` offer is really
/// plain text.
#[must_use]
pub fn is_nautilus_text_clipboard(bytes: &[u8]) -> bool {
    let Some(head) = bytes.get(..NAUTILUS_TEXT_MAGIC.len()) else {
        return false;
    };
    head == NAUTILUS_TEXT_MAGIC.as_bytes()
}

/// Read an `application/x-kde-cutselection` payload.
///
/// Infallible, and looks at byte 0 only, because that is precisely what
/// `KIO::isClipboardDataCut` does. An absent, empty or unrecognized payload is
/// [`FileAction::Copy`] — the safe reading, since guessing "cut" would move a
/// user's files.
#[must_use]
pub fn parse_kde_cut_selection(bytes: &[u8]) -> FileAction {
    match bytes.first() {
        Some(b'1') => FileAction::Cut,
        _ => FileAction::Copy,
    }
}

/// Match a verb line, case-insensitively and ignoring surrounding whitespace.
fn action_of(line: &str) -> Option<FileAction> {
    let t = line.trim();
    if t.eq_ignore_ascii_case("copy") {
        Some(FileAction::Copy)
    } else if t.eq_ignore_ascii_case("cut") {
        Some(FileAction::Cut)
    } else {
        None
    }
}

//! Windows `.url` — the `[InternetShortcut]` file.
//!
//! A `.url` is what Explorer writes when you drag a browser's address bar onto
//! the desktop, and what lands on the clipboard as a file when you copy one.
//! It is an INI-shaped text file whose only required key is
//! `[InternetShortcut] URL=`.
//!
//! # There is no specification
//!
//! Microsoft never published one. This crate is written against:
//!
//! - [An Unofficial Guide to the URL File Format](https://www.cyanwerks.com/formats/file-format-url.html)
//!   (Edward L. Blake, 3rd ed.) — the key list, the `HotKey` table, and the
//!   `Modified` FILETIME encoding.
//! - Wine's `dlls/ieframe/intshcut.c` — an actual reimplementation of
//!   `IUniformResourceLocator` / `IPersistFile`, which is where the
//!   case-insensitivity and CRLF-and-UTF-8 behaviour in [`ini`] come from.
//!
//! Treat every field description here as "observed", not "specified".
//!
//! # What this crate will not do
//!
//! It returns data. It does not resolve `IconFile`, open `URL`, decode
//! `IDList`, or look at the filesystem — a `.url` arriving over the clipboard
//! is written by another process and is hostile until proven otherwise.
//!
//! # Example
//!
//! ```
//! # use rclip_url_file::{parse, ShortcutTarget};
//! let bytes = b"[InternetShortcut]\r\nURL=https://example.com/\r\nIconIndex=3\r\n";
//! let f = parse(bytes).unwrap();
//! assert_eq!(f.url(), Some("https://example.com/"));
//! assert_eq!(f.target(), Some(ShortcutTarget::Url("https://example.com/")));
//! assert_eq!(f.icon_index().unwrap().unwrap(), 3);
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "codepage")]
pub mod codepage;
pub mod fields;
pub mod ini;
mod lines;
pub mod shortcut;

use rclip_core::{Error, ErrorKind, Reader, Result};

pub use fields::{HotKey, Modified, ShowCommand};
pub use ini::{Entries, Entry, Section, Sections};
/// Re-exported so a caller can name a code page without adding `rclip-codepage`
/// to its own manifest.
#[cfg(feature = "codepage")]
pub use rclip_codepage::Encoding;
pub use shortcut::ShortcutTarget;

/// The section every `.url` has. Compared case-insensitively; see [`ini`].
pub const SECTION_INTERNET_SHORTCUT: &str = "InternetShortcut";

/// The "ANSI" companion section. Its `URL=` is believed to be the value in the
/// system code page, but nobody has documented it — the NSIS wiki's own note
/// reads `[InternetShortcut.A] ; CP_ACP stuff?`. [`UrlFile::url_ansi`] hands the
/// value back verbatim and decodes nothing.
///
/// A file that actually uses this section is not UTF-8, so [`parse`] refuses it
/// outright. The `codepage` feature's `codepage` module is the way in: transcode
/// the bytes with a code page the caller names, then parse.
pub const SECTION_INTERNET_SHORTCUT_A: &str = "InternetShortcut.A";

/// The "wide" companion section. Same story: the NSIS wiki guesses
/// `; UTF-7 stuff?` and no primary source confirms it. Verbatim, undecoded.
pub const SECTION_INTERNET_SHORTCUT_W: &str = "InternetShortcut.W";

/// A parsed `.url` file.
///
/// Holds a borrowed view of the caller's buffer; nothing is copied and nothing
/// is decoded until an accessor asks for it.
#[derive(Debug, Copy, Clone)]
pub struct UrlFile<'a> {
    src: &'a str,
    /// Offset of `src` within the original byte buffer — 3 when a UTF-8 BOM was
    /// stripped, 0 otherwise. Every reported error offset is relative to the
    /// buffer the caller passed, not to `src`.
    base: usize,
}

/// Parse a `.url` file.
///
/// # Errors
///
/// - [`ErrorKind::InvalidUtf8`] if the bytes are not UTF-8. Real files are
///   ASCII (a `URL=` is percent-encoded) and Wine writes UTF-8; a legacy file
///   in a Windows code page must be transcoded by the caller first.
/// - [`ErrorKind::Malformed`] for an unterminated `[section` header or a
///   `key=value` line before the first section. See [`ini::validate`] for why
///   those two and nothing else.
pub fn parse(bytes: &[u8]) -> Result<UrlFile<'_>> {
    let (src, base) = decode(bytes)?;
    ini::validate(src, base)?;
    Ok(UrlFile { src, base })
}

/// Validate the input as UTF-8 and step over a byte-order mark.
///
/// A BOM matters here: `\u{FEFF}[InternetShortcut]` does not start with `[`,
/// so without this the first section header is not a header and the whole file
/// reads as empty. Text editors on Windows add one by default.
fn decode(bytes: &[u8]) -> Result<(&str, usize)> {
    let mut r = Reader::new(bytes);
    let base = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        r.skip(3)?;
        3
    } else {
        0
    };
    let rest = r.remaining();
    let src = core::str::from_utf8(rest).map_err(|e| {
        // `valid_up_to` is relative to `rest`; report it against the caller's
        // buffer so the offset matches a hex dump of the file.
        Error::new(ErrorKind::InvalidUtf8, base + e.valid_up_to())
    })?;
    Ok((src, base))
}

impl<'a> UrlFile<'a> {
    /// The file text, BOM excluded.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        self.src
    }

    /// Every section, in file order.
    #[must_use]
    pub const fn sections(&self) -> Sections<'a> {
        Sections::new(self.src, self.base)
    }

    /// The first section with this name, compared ASCII-case-insensitively.
    #[must_use]
    pub fn section(&self, name: &str) -> Option<Section<'a>> {
        self.sections().find(|s| s.is(name))
    }

    /// The `[InternetShortcut]` section.
    #[must_use]
    pub fn internet_shortcut(&self) -> Option<Section<'a>> {
        self.section(SECTION_INTERNET_SHORTCUT)
    }

    fn key(&self, key: &str) -> Option<Entry<'a>> {
        self.internet_shortcut()?
            .entries()
            .find(|e| e.key.eq_ignore_ascii_case(key))
    }

    /// `URL=` — the only key the format requires.
    #[must_use]
    pub fn url(&self) -> Option<&'a str> {
        self.key("URL").map(|e| e.value)
    }

    /// `URL=`, or [`ErrorKind::Malformed`] if the file does not have one.
    ///
    /// Kept separate from [`UrlFile::url`] so that [`parse`] can stay
    /// structural: a `.url` with a `[InternetShortcut.W]` but no plain
    /// `[InternetShortcut]` is odd but readable, and rejecting it at parse time
    /// would throw away the rest of the file.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] when `URL=` is absent.
    pub fn require_url(&self) -> Result<&'a str> {
        self.url()
            .ok_or(Error::new(ErrorKind::Malformed, self.base))
    }

    /// Where the shortcut points, classified.
    ///
    /// `.url` files overwhelmingly carry a real URL, but `URL=file:///C:/x` and
    /// even a bare `URL=C:\x` both occur, which is why this goes through
    /// [`ShortcutTarget::classify`] rather than assuming.
    #[must_use]
    pub fn target(&self) -> Option<ShortcutTarget<'a>> {
        self.url().map(ShortcutTarget::classify)
    }

    /// `[InternetShortcut.A] URL=`, verbatim and undecoded. See
    /// [`SECTION_INTERNET_SHORTCUT_A`].
    #[must_use]
    pub fn url_ansi(&self) -> Option<&'a str> {
        self.section(SECTION_INTERNET_SHORTCUT_A)?.get("URL")
    }

    /// `[InternetShortcut.W] URL=`, verbatim and undecoded. See
    /// [`SECTION_INTERNET_SHORTCUT_W`].
    #[must_use]
    pub fn url_wide(&self) -> Option<&'a str> {
        self.section(SECTION_INTERNET_SHORTCUT_W)?.get("URL")
    }

    /// `IconFile=` — a path to an icon container (`.ico`, `.exe`, `.dll`).
    /// Not resolved, not opened.
    #[must_use]
    pub fn icon_file(&self) -> Option<&'a str> {
        self.key("IconFile").map(|e| e.value)
    }

    /// `IconIndex=` — index into [`UrlFile::icon_file`].
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] if the value is not a number,
    /// [`ErrorKind::TooLarge`] if it does not fit an `i32`.
    #[must_use]
    pub fn icon_index(&self) -> Option<Result<i32>> {
        self.key("IconIndex")
            .map(|e| fields::int(e.value, e.offset))
    }

    /// `HotKey=` — see [`HotKey`] for the bit layout.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] if the value is not a number,
    /// [`ErrorKind::TooLarge`] if it does not fit the 16-bit field.
    #[must_use]
    pub fn hotkey(&self) -> Option<Result<HotKey>> {
        self.key("HotKey").map(|e| {
            let raw = fields::uint(e.value, e.offset)?;
            let word = u16::try_from(raw).map_err(|_| Error::new(ErrorKind::TooLarge, e.offset))?;
            Ok(HotKey::from_word(word))
        })
    }

    /// `ShowCommand=` — the `SW_*` state to open the target with.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Malformed`] if the value is not a number.
    #[must_use]
    pub fn show_command(&self) -> Option<Result<ShowCommand>> {
        self.key("ShowCommand")
            .map(|e| fields::uint(e.value, e.offset).map(ShowCommand))
    }

    /// `Modified=` — hex `FILETIME`. See [`Modified`].
    ///
    /// # Errors
    ///
    /// [`ErrorKind::BadLength`] for fewer than sixteen hex digits,
    /// [`ErrorKind::Malformed`] for a non-hex digit.
    #[must_use]
    pub fn modified(&self) -> Option<Result<Modified<'a>>> {
        self.key("Modified")
            .map(|e| fields::modified(e.value, e.offset))
    }

    /// `WorkingDirectory=` — verbatim, never resolved.
    #[must_use]
    pub fn working_directory(&self) -> Option<&'a str> {
        self.key("WorkingDirectory").map(|e| e.value)
    }

    /// `IDList=` — an encoded shell `ITEMIDLIST`, handed back as written.
    ///
    /// Almost always empty in files on disk. Decoding it needs a PIDL parser.
    //
    // TODO(phase-3): decode through `rclip-idlist` once that crate exists.
    #[must_use]
    pub fn id_list(&self) -> Option<&'a str> {
        self.key("IDList").map(|e| e.value)
    }
}

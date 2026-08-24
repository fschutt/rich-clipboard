//! Reading a `.url` that is not UTF-8.
//!
//! [`crate::parse`] requires UTF-8, and that is the right default: a real `.url`
//! is ASCII, because a `URL=` is percent-encoded, and Wine writes UTF-8. But the
//! `[InternetShortcut.A]` section exists precisely to hold the *un-encoded* URL
//! in the writing machine's ANSI code page, and a file that uses it is not UTF-8
//! at all — `parse` refuses the whole file with
//! [`ErrorKind::InvalidUtf8`](rclip_core::ErrorKind::InvalidUtf8), not just that
//! one value.
//!
//! So the fix is not a decoding accessor on [`UrlFile`], it is a transcode step
//! in front of the parser. Every encoding `rclip-codepage` implements is
//! ASCII-transparent, so transcoding a file whose other sections are ASCII
//! leaves them byte-identical; only the high bytes change.
//!
//! Nothing here guesses. `enc` is a parameter because the code page is not in
//! the file: the NSIS wiki's own annotation for this section is
//! `[InternetShortcut.A] ; CP_ACP stuff?`, question mark in the original.

use alloc::string::String;

use rclip_codepage::Encoding;
use rclip_core::Result;

use crate::{UrlFile, SECTION_INTERNET_SHORTCUT_A};

/// Transcode a `.url` file out of a legacy code page, so [`crate::parse`] can
/// read it.
///
/// A UTF-8 BOM is *not* stripped here — [`crate::parse`] does that, and doing it
/// twice would eat three real characters from a file that happens to start with
/// `ï»¿`. Feed the result straight to `parse`.
///
/// # Errors
///
/// [`ErrorKind::Malformed`](rclip_core::ErrorKind::Malformed) at the offset of a
/// byte `enc` leaves undefined. That is a strong signal the wrong code page was
/// named, which is worth an error rather than a replacement character in a URL.
pub fn decode(bytes: &[u8], enc: Encoding) -> Result<String> {
    enc.decode_to_string(bytes)
}

/// Transcode a `.url` file, substituting U+FFFD for undefined bytes.
///
/// For a URL this is nearly always the wrong trade — a U+FFFD in a host name
/// produces a link that looks readable and does not resolve — so prefer
/// [`decode`] and surface the failure. This exists for the display case.
#[must_use]
pub fn decode_lossy(bytes: &[u8], enc: Encoding) -> String {
    enc.decode_to_string_lossy(bytes)
}

/// `[InternetShortcut.A] URL=`, decoded, from an undecoded byte buffer.
///
/// The one-liner for the case this module exists for: transcode, parse, and
/// pull the ANSI section's `URL=`. `Ok(None)` means the file parsed and has no
/// `[InternetShortcut.A]` section, which is the common case — the section is
/// optional and most writers omit it.
///
/// # Errors
///
/// Whatever [`decode`] and [`crate::parse`] return: a byte undefined in `enc`,
/// or a structurally malformed file.
pub fn url_ansi(bytes: &[u8], enc: Encoding) -> Result<Option<String>> {
    let text = decode(bytes, enc)?;
    let file = crate::parse(text.as_bytes())?;
    Ok(section_a_url(&file).map(String::from))
}

/// `[InternetShortcut.A] URL=` of an already-parsed file.
fn section_a_url<'a>(file: &UrlFile<'a>) -> Option<&'a str> {
    file.section(SECTION_INTERNET_SHORTCUT_A)?.get("URL")
}

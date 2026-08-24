//! Which payloads to publish, and how to build them byte-exactly.
//!
//! There is no single Linux clipboard format for "these files, cut". There are
//! three families that do not read each other's, so a source that wants a cut
//! to survive has to offer all of them. [`RECOMMENDED`] is that list.
//!
//! Byte-exactness is not cosmetic here. Since Nautilus 44 the
//! `x-special/gnome-copied-files` reader rejects the entire payload if any line
//! after the verb is empty or fails `g_uri_is_valid` — so a trailing newline
//! makes the paste do nothing at all. `text/uri-list`, in contrast, is written
//! with CRLF *and* a trailing CRLF by both GTK (`gdkcontentserializer.c`) and
//! Qt (`QMimeData::retrieveTypedData`), and every reader tolerates its absence.
//! The two formats therefore differ in line ending and in trailing terminator,
//! and [`write_uri_list`] and [`write_copied_files`] differ with them.

use crate::convention::{
    FileAction, MIME_GNOME_COPIED_FILES, MIME_KDE_CUT_SELECTION, MIME_MATE_COPIED_FILES,
    MIME_URI_LIST,
};

/// How to build the bytes for one offered MIME type.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Payload {
    /// RFC 2483 list, CRLF-terminated, no verb. Carries the files.
    UriList,
    /// `verb LF uri (LF uri)*`, no trailing newline. Carries files and verb.
    CopiedFiles,
    /// One byte, `1` or `0`. Carries the verb only; pair it with
    /// [`Payload::UriList`].
    KdeCutSelection,
}

/// One MIME type to publish and the shape of its bytes.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Offer {
    /// The MIME type / X11 target / Wayland type string.
    pub mime: &'static str,
    /// What to put in it.
    pub payload: Payload,
}

/// Everything a file-copy source should offer, in the order to advertise it.
///
/// `text/uri-list` comes first because it is the only one every reader
/// understands; a receiver that knows nothing about verbs still gets the files.
///
/// Deliberately **not** in this list: `x-special/nautilus-clipboard`. It was
/// never a MIME type — it was a magic first line inside Nautilus's
/// `text/plain` payload — and Nautilus stopped writing it in version 40
/// (commit `2045f662`, 2021-03). Offering it would mean publishing a `text/plain`
/// that is not text. [`crate::convention::parse_nautilus_text_clipboard`] still
/// reads it, because old payloads outlive the code that made them.
pub const RECOMMENDED: [Offer; 4] = [
    Offer {
        mime: MIME_URI_LIST,
        payload: Payload::UriList,
    },
    Offer {
        mime: MIME_GNOME_COPIED_FILES,
        payload: Payload::CopiedFiles,
    },
    // MATE's Caja uses its own name for a byte-identical payload.
    Offer {
        mime: MIME_MATE_COPIED_FILES,
        payload: Payload::CopiedFiles,
    },
    Offer {
        mime: MIME_KDE_CUT_SELECTION,
        payload: Payload::KdeCutSelection,
    },
];

/// The `application/x-kde-cutselection` bytes. No allocation needed — the
/// payload is one byte and both values are static.
#[must_use]
pub const fn kde_cut_selection(action: FileAction) -> &'static [u8] {
    action.kde_payload()
}

#[cfg(feature = "alloc")]
mod with_alloc {
    extern crate alloc;

    use alloc::vec::Vec;

    use super::{Offer, Payload};
    use crate::convention::FileAction;

    /// Build a `text/uri-list` body.
    ///
    /// CRLF after every URI, last one included. That is what GTK and Qt emit,
    /// and RFC 2483 §5 asks for CRLF; readers that only split on LF still cope,
    /// because they trim.
    ///
    /// URIs are written through **verbatim**. That is deliberate and is not the
    /// same thing as "this crate cannot encode": a `&str` handed to this
    /// function is already a URI, and re-encoding one would double every `%`
    /// it already contains. Turn a *path* into a URI with [`file_uri`] first,
    /// which is the direction where the escaping question has an answer.
    /// # A leading BOM
    ///
    /// If the first URI itself starts with U+FEFF, this writes a BOM *before*
    /// it. That looks redundant and is not: [`crate::parse`] strips a BOM at
    /// offset 0 as an encoding mark, so a payload whose first bytes are a
    /// content BOM comes back one character shorter and, if that was the whole
    /// URI, one URI shorter. Emitting the encoding mark explicitly pushes the
    /// content BOM to offset 3 where it survives, which is the ordinary way a
    /// BOM-aware text format escapes a leading BOM.
    ///
    /// Nothing else is affected — the prefix appears only when the output would
    /// otherwise open with a BOM. U+FEFF cannot occur in a real URI anyway
    /// (RFC 3986 URIs are ASCII), so in practice this fires only for a "URI"
    /// that a lenient read produced from junk. Found by the `uri_list_parse`
    /// fuzz target, whose round-trip property it was violating.
    #[must_use]
    pub fn write_uri_list<'a, I>(uris: I) -> Vec<u8>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut out = Vec::new();
        for uri in uris {
            if out.is_empty() && uri.as_bytes().starts_with(crate::BOM) {
                out.extend_from_slice(crate::BOM);
            }
            out.extend_from_slice(uri.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out
    }

    /// Build an `x-special/gnome-copied-files` (or `…mate-copied-files`) body.
    ///
    /// Verb, then LF before each URI. **No trailing newline**, LF and never
    /// CRLF: Nautilus 44+ rejects a payload containing an empty line, and
    /// splitting `"file:///a\r\n"` on `\n` leaves `"file:///a\r"`, which fails
    /// `g_uri_is_valid`. Either mistake makes the paste silently do nothing.
    #[must_use]
    pub fn write_copied_files<'a, I>(action: FileAction, uris: I) -> Vec<u8>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut out = Vec::new();
        out.extend_from_slice(action.verb().as_bytes());
        for uri in uris {
            out.push(b'\n');
            out.extend_from_slice(uri.as_bytes());
        }
        out
    }

    /// Build a `file://` URI for an absolute path.
    ///
    /// The path is percent-encoded with [`EncodeSet::Path`], so `/` stays a
    /// separator and everything that would change the URI's meaning — a space,
    /// a `#`, a `?`, a literal `%`, a control byte, anything non-ASCII — is
    /// escaped. See [`EncodeSet`] for why the set is neither larger nor
    /// smaller than that.
    ///
    /// Takes bytes, so a POSIX path that is not UTF-8 survives; `&str` works
    /// too. A leading `/` is supplied if the path lacks one, because
    /// `file://home/me` parses `home` as an *authority* — a hostname — and not
    /// as the first path component. That is the one malformation that changes
    /// what the URI means rather than merely how it looks, and it is a
    /// syntactic category error rather than path resolution, which this crate
    /// does not do.
    ///
    /// ```
    /// use rclip_uri_list::emit::file_uri;
    ///
    /// assert_eq!(file_uri("/home/me/a file.txt"), "file:///home/me/a%20file.txt");
    /// // Sub-delims are legal in a path and are left alone, exactly as
    /// // `g_filename_to_uri` leaves them.
    /// assert_eq!(file_uri("/tmp/it's (1)&2.txt"), "file:///tmp/it's%20(1)&2.txt");
    /// ```
    ///
    /// [`EncodeSet`]: crate::uri::EncodeSet
    /// [`EncodeSet::Path`]: crate::uri::EncodeSet::Path
    #[must_use]
    pub fn file_uri<P: AsRef<[u8]>>(path: P) -> alloc::string::String {
        use crate::uri::{percent_encode, EncodeSet};
        use core::fmt::Write as _;

        let path = path.as_ref();
        let mut out = alloc::string::String::with_capacity(
            "file:///".len() + crate::uri::encoded_len(path, EncodeSet::Path),
        );
        out.push_str("file://");
        if !path.starts_with(b"/") {
            out.push('/');
        }
        // Infallible: writing into a String cannot fail, and `PercentEncode`
        // yields nothing but ASCII.
        let _ = write!(out, "{}", percent_encode(path, EncodeSet::Path));
        out
    }

    /// Build the bytes for one [`Offer`].
    ///
    /// Lets a source loop over [`super::RECOMMENDED`] without matching on the
    /// payload kind itself.
    #[must_use]
    pub fn write(offer: Offer, action: FileAction, uris: &[&str]) -> Vec<u8> {
        match offer.payload {
            Payload::UriList => write_uri_list(uris.iter().copied()),
            Payload::CopiedFiles => write_copied_files(action, uris.iter().copied()),
            Payload::KdeCutSelection => action.kde_payload().to_vec(),
        }
    }
}

#[cfg(feature = "alloc")]
pub use with_alloc::{file_uri, write, write_copied_files, write_uri_list};

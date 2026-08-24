//! Reader for macOS `.webloc` and `.inetloc` internet location files.
//!
//! A `.webloc` is a property list whose single dictionary has the key `URL`.
//! `.inetloc` adds `URLName`, the human-readable title. Both encodings turn up
//! in the wild and this crate reads either:
//!
//! - **XML**, which is what Finder's `make new internet location file` writes.
//! - **`bplist00`**, the binary plist, which is what a link dragged from a
//!   browser to the Finder produces.
//!
//! Sniff on the first eight bytes: `bplist00` means binary, a `<` after
//! optional whitespace and BOM means XML.
//!
//! There is a third, older form: a file with an **empty data fork** whose
//! resource fork holds `url ` and `drag` resources. Pre-OS X internet location
//! files were written that way, and Finder still writes those resources
//! alongside the plist today — the capture in
//! `corpus/synthetic/rclip-webloc/finder-created.webloc` has both. It is out of
//! scope for phase 0 and would need a resource-fork reader, which is a format
//! of its own.
//!
//! # Strings come back as [`Text`], not `&str`
//!
//! Because parsing borrows and does not allocate, and the value is not always
//! UTF-8 sitting there ready to be borrowed:
//!
//! - An XML value can contain entity references. CoreFoundation writes `&` as
//!   `&amp;`, so a URL with two query parameters *always* arrives escaped;
//!   handing back the raw slice would be wrong in the most ordinary case there
//!   is.
//! - A binary plist string is either `0x5n` ASCII or `0x6n` **UTF-16
//!   big-endian**, the latter for anything with a non-ASCII character in it.
//!
//! [`Text`] knows which, iterates as `char`s in every case, and grows a
//! `to_string_lossy` behind the `alloc` feature.
//!
//! # Example
//!
//! ```
//! # fn main() -> Result<(), rclip_core::Error> {
//! # let bytes = include_bytes!("../../../corpus/synthetic/rclip-webloc/finder-created.webloc");
//! let loc = rclip_webloc::Webloc::parse(bytes)?;
//! assert_eq!(loc.encoding(), rclip_webloc::Encoding::Xml);
//! assert!(loc.url().eq_str("https://example.com/rich-clipboard"));
//! # Ok(())
//! # }
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod bplist;
mod text;
pub mod xml;

use rclip_core::{Error, ErrorKind, Result};

pub use bplist::BinaryPlist;
pub use text::{Chars, Text};

/// The dictionary key that makes a file a `.webloc`.
pub const KEY_URL: &str = "URL";
/// The extra key a `.inetloc` carries: the link's display title.
pub const KEY_URL_NAME: &str = "URLName";

/// Which of the two plist encodings the file uses.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Encoding {
    /// An XML property list. What Finder's scripting interface writes.
    Xml,
    /// A `bplist00` binary property list. What a drag from a browser writes.
    Binary,
}

/// A parsed internet location file.
#[derive(Debug, Copy, Clone)]
pub struct Webloc<'a> {
    encoding: Encoding,
    url: Text<'a>,
    url_name: Option<Text<'a>>,
}

impl<'a> Webloc<'a> {
    /// Sniff the encoding without parsing.
    ///
    /// `None` for anything that is neither — a text file someone renamed, or
    /// the resource-fork-only form described under *What is not implemented*.
    #[must_use]
    pub fn detect(buf: &[u8]) -> Option<Encoding> {
        // TODO(phase-4): the legacy resource-fork form, whose data fork is
        // empty and whose `url ` resource holds the URL. Detecting it needs the
        // resource fork, which is not in this buffer — on macOS it is the
        // `..namedfork/rsrc` stream, and in an archive it is an AppleDouble
        // sidecar. Both are a separate reader.
        if BinaryPlist::detect(buf) {
            Some(Encoding::Binary)
        } else if xml::detect(buf) {
            Some(Encoding::Xml)
        } else {
            None
        }
    }

    /// Parse a `.webloc` or `.inetloc`.
    ///
    /// Fails with [`ErrorKind::Malformed`] if the plist is well-formed but has
    /// no `URL` key. That key is the whole content of the format, so a file
    /// without it is not a partially readable location file — it is a different
    /// document that happens to be a plist.
    pub fn parse(buf: &'a [u8]) -> Result<Self> {
        match Self::detect(buf) {
            Some(Encoding::Binary) => Self::parse_binary(buf),
            Some(Encoding::Xml) => Self::parse_xml(buf),
            None => Err(Error::new(ErrorKind::BadMagic, 0)),
        }
    }

    fn parse_xml(buf: &'a [u8]) -> Result<Self> {
        let doc = xml::as_str(buf)?;
        let mut url = None;
        let mut url_name = None;
        for pair in xml::Entries::new(doc) {
            let (key, value) = pair?;
            if url.is_none() && key.eq_str(KEY_URL) {
                url = Some(value);
            } else if url_name.is_none() && key.eq_str(KEY_URL_NAME) {
                url_name = Some(value);
            }
        }
        Ok(Self {
            encoding: Encoding::Xml,
            url: url.ok_or(Error::new(ErrorKind::Malformed, 0))?,
            url_name,
        })
    }

    fn parse_binary(buf: &'a [u8]) -> Result<Self> {
        let plist = BinaryPlist::parse(buf)?;
        let bplist::Object::Dict { keys, values, count } = plist.object(plist.top_object(), 0)?
        else {
            // The root of a location file is a dictionary. An array or a bare
            // string at the root is a valid plist and not a .webloc.
            return Err(Error::new(ErrorKind::Malformed, 0));
        };

        let mut url = None;
        let mut url_name = None;
        for i in 0..count {
            let bplist::Object::Str(key) = plist.object(plist.reference(keys, i)?, 1)? else {
                // A non-string key is legal in a plist and meaningless here.
                continue;
            };
            let wanted = if url.is_none() && key.eq_str(KEY_URL) {
                &mut url
            } else if url_name.is_none() && key.eq_str(KEY_URL_NAME) {
                &mut url_name
            } else {
                // Values for keys nobody asked about are never resolved, so a
                // malformed one costs nothing.
                continue;
            };
            match plist.object(plist.reference(values, i)?, 1)? {
                bplist::Object::Str(v) => *wanted = Some(v),
                // A container where a string belongs — including a reference
                // back to the root dictionary, which is how a hostile file
                // spells "recurse forever".
                _ => return Err(Error::new(ErrorKind::Unsupported, 0)),
            }
        }

        Ok(Self {
            encoding: Encoding::Binary,
            url: url.ok_or(Error::new(ErrorKind::Malformed, 0))?,
            url_name,
        })
    }

    /// Which encoding the file used.
    #[must_use]
    pub const fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// The `URL` value. Always present — [`Webloc::parse`] fails without it.
    #[must_use]
    pub const fn url(&self) -> Text<'a> {
        self.url
    }

    /// The `URLName` value: the display title a `.inetloc` carries, and that a
    /// plain `.webloc` does not.
    #[must_use]
    pub const fn url_name(&self) -> Option<Text<'a>> {
        self.url_name
    }
}

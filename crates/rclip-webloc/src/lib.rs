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
//! - **A resource fork**, holding a `url ` resource. Pre-OS X internet location
//!   files had an empty data fork and lived entirely in this form, and Finder
//!   still writes those resources alongside the plist today: the capture in
//!   `corpus/synthetic/rclip-webloc/finder-created.bin` is the data fork of a
//!   file whose resource fork is `corpus/macos/finder/webloc-resource-fork.bin`.
//!   [`rsrc`] reads it.
//!
//! A resource fork is a separate stream and does not travel in the file's
//! bytes — on macOS it is `<file>/..namedfork/rsrc`, in an archive it is an
//! AppleDouble sidecar, and on the clipboard it does not travel at all.
//! [`Webloc::parse`] takes whichever stream you hand it and works out which of
//! the three it is; see [`Webloc::detect`].
//!
//! The other thing that marks such a file is its `com.apple.FinderInfo`
//! extended attribute, whose four-character type is `il` plus two characters
//! for the scheme and whose creator is `MACS`. That is not in either fork
//! either; [`is_internet_location_finder_info`] checks it when a caller has it.
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
//! # let bytes = include_bytes!("../../../corpus/synthetic/rclip-webloc/finder-created.bin");
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
pub mod rsrc;
mod text;
pub mod xml;

use rclip_core::{Error, ErrorKind, Result};

pub use bplist::BinaryPlist;
pub use rsrc::{Resource, ResourceFork, ResourceType};
pub use text::{Chars, Text};

/// The dictionary key that makes a file a `.webloc`.
pub const KEY_URL: &str = "URL";
/// The extra key a `.inetloc` carries: the link's display title.
pub const KEY_URL_NAME: &str = "URLName";

/// The creator code of every internet location file macOS writes: `MACS`.
pub const FINDER_CREATOR: [u8; 4] = *b"MACS";

/// `true` if a 32-byte `com.apple.FinderInfo` value describes an internet
/// location file.
///
/// The check is `creator == MACS` and a type code beginning `il` — "internet
/// location", with the last two characters naming the scheme. Observed on
/// macOS 15.5, by asking Finder to write one of each:
///
/// | Type | Extension | URL |
/// |---|---|---|
/// | `ilht` | `.webloc` | `https://…` |
/// | `ilft` | `.ftploc` | `ftp://…` |
/// | `ilma` | `.mailloc` | `mailto:…` |
/// | `ilfi` | `.fileloc` | `file://…` |
/// | `ilaf` | `.afploc` | `afp://…` |
/// | `ilnw` | `.nntploc` | `news:…` |
///
/// The prefix is matched rather than that list, because the list is open: the
/// last two characters follow the scheme, and a scheme this table does not
/// mention still produces an internet location file.
///
/// This is not in either fork. `FinderInfo` is an extended attribute, and
/// fetching it is the caller's business — but for the legacy form it is the
/// only thing that says what an empty data fork was, so this crate knows how to
/// read the answer. Anything shorter than the first eight bytes is `false`.
#[must_use]
pub fn is_internet_location_finder_info(finder_info: &[u8]) -> bool {
    let Some(head) = finder_info.get(..8) else {
        return false;
    };
    head.starts_with(b"il") && head[4..8] == FINDER_CREATOR
}

/// Which of the two plist encodings the file uses.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Encoding {
    /// An XML property list. What Finder's scripting interface writes.
    Xml,
    /// A `bplist00` binary property list. What a drag from a browser writes.
    Binary,
    /// A Macintosh resource fork holding a `url ` resource. The pre-OS X form,
    /// which Finder still writes alongside the plist. See [`rsrc`].
    ResourceFork,
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
    /// `None` for anything that is none of the three — a text file someone
    /// renamed, or an empty data fork whose resource fork was not handed over.
    ///
    /// The two plist encodings have magic and go first. A resource fork has
    /// none: it opens with four offsets, so recognising one means checking that
    /// those four agree with the buffer and that the resource map's own two
    /// offsets agree with the map. That check is strict enough that text cannot
    /// pass it — the first four bytes of any text are a data offset in the
    /// hundreds of millions — but it is a structural verdict rather than a
    /// signature, which is why it is tried last.
    #[must_use]
    pub fn detect(buf: &[u8]) -> Option<Encoding> {
        if BinaryPlist::detect(buf) {
            Some(Encoding::Binary)
        } else if xml::detect(buf) {
            Some(Encoding::Xml)
        } else if ResourceFork::detect(buf) {
            Some(Encoding::ResourceFork)
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
            Some(Encoding::ResourceFork) => Self::parse_resource_fork(buf),
            None => Err(Error::new(ErrorKind::BadMagic, 0)),
        }
    }

    /// Parse the legacy resource-fork form: a fork with a `url ` resource.
    ///
    /// `buf` is the **resource fork**, not the file. On macOS that is the
    /// `<file>/..namedfork/rsrc` stream; see [`rsrc`].
    ///
    /// Fails with [`ErrorKind::Malformed`] when the fork parses and has no
    /// `url ` resource, for the same reason the plist form fails without a
    /// `URL` key: that resource is the whole content of the format, so a fork
    /// without one is a different document that happens to be a resource fork.
    ///
    /// # No `URLName`
    ///
    /// This form carries no title. Finder puts the name in the *filename* and
    /// writes no `urln` resource — confirmed against files it wrote for `http`,
    /// `mailto`, `ftp`, `afp`, `file` and `news` URLs, every one of which held
    /// exactly `drag`, `TEXT` and `url `. [`Webloc::url_name`] is therefore
    /// always `None` here, rather than guessed at from the `TEXT` resource,
    /// which holds the URL again and not a title.
    pub fn parse_resource_fork(buf: &'a [u8]) -> Result<Self> {
        let fork = ResourceFork::parse(buf)?;
        let res = fork
            .first_resource(rsrc::TYPE_URL)
            .ok_or(Error::new(ErrorKind::Malformed, 0))??;
        // A compressed resource's bytes are a compressed image, not the URL.
        // Handing them back as text would be confidently wrong.
        if res.is_compressed() {
            return Err(Error::new(ErrorKind::Unsupported, 0));
        }
        // The resource is text in the writing machine's system encoding, which
        // the fork does not record. A URL is ASCII by RFC 3986 and every
        // capture is, so valid UTF-8 is the whole of the real world; anything
        // else needs a code page nobody wrote down. The bytes stay reachable
        // through `rsrc` for a caller that knows better.
        let url = core::str::from_utf8(res.data)
            .map_err(|e| Error::new(ErrorKind::InvalidUtf8, e.valid_up_to()))?;

        Ok(Self {
            encoding: Encoding::ResourceFork,
            // Never XML-escaped: a resource is bytes, not markup.
            url: Text::Utf8(url),
            url_name: None,
        })
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
        let bplist::Object::Dict {
            keys,
            values,
            count,
        } = plist.object(plist.top_object(), 0)?
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

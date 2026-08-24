//! `CF_HDROP` — the Win32 clipboard format for "here is a list of existing files".
//!
//! The payload is a [`DROPFILES`] header followed, at a byte offset the header
//! itself names, by a double-NUL-terminated array of paths. This crate parses
//! that and builds it, because `CF_HDROP` is the format files travel on in
//! *both* directions: it is what Explorer puts on the clipboard when you press
//! Ctrl+C on a selection, and what your application must produce to be pasted
//! into Explorer.
//!
//! ```text
//! offset 0   pFiles : DWORD   byte offset to the path array, from offset 0
//!        4   pt.x   : LONG    drop point
//!        8   pt.y   : LONG
//!       12   fNC    : BOOL    pt is in screen (nonclient) rather than client coords
//!       16   fWide  : BOOL    path array is UTF-16LE rather than system ANSI
//!       20   …               (pFiles usually, but not necessarily, points here)
//! ```
//!
//! [`DROPFILES`]: https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-dropfiles
//!
//! # Parsing
//!
//! ```
//! use rclip_dropfiles::{DropFiles, Path};
//!
//! # fn main() -> Result<(), rclip_core::Error> {
//! // pFiles=20, pt=(0,0), fNC=0, fWide=1, then "A:\a\0" and the array NUL.
//! const PAYLOAD: &[u8] = &[
//!     20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0,
//!     b'A', 0, b':', 0, b'\\', 0, b'a', 0, 0, 0, // "A:\a" + NUL
//!     0, 0, // array terminator
//! ];
//!
//! let drop = DropFiles::parse(PAYLOAD)?;
//! assert!(drop.is_wide());
//! // `path` borrows PAYLOAD; parsing allocated nothing.
//! let names: Vec<Path<'_>> = drop.paths().collect();
//! assert_eq!(names.len(), 1);
//! assert_eq!(names[0].as_bytes(), b"A\0:\0\\\0a\0");
//! # Ok(())
//! # }
//! ```
//!
//! # ANSI paths
//!
//! `fWide == 0` selects the *system ANSI* code page, which is a property of the
//! machine the bytes came from and is not recorded anywhere in the payload.
//! [`Path::Ansi`] therefore hands back the raw bytes and refuses to guess:
//! [`Path::chars`] and [`Path::to_string_lossy`] both return `None`.
//!
//! The optional, default-off `codepage` feature adds the API for a caller that
//! *knows* the code page — from `CF_LOCALE`, from the transport, from the user:
//! `Path::chars_with`, `Path::to_string_with` and `Builder::push_str_encoded`
//! all take an `Encoding` rather than assuming one. Guessing Windows-1252 as a
//! default would silently corrupt every non-Latin path, so the parameter is
//! required and there is no default.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use rclip_core::{Error, ErrorKind, Reader, Result, Utf16Le};

/// Size of the `DROPFILES` header, and the value `pFiles` almost always has.
///
/// `DWORD + POINT + BOOL + BOOL` = 4 + 8 + 4 + 4. Every member is 4-byte
/// aligned so the compiler inserts no padding; the wire layout is the
/// declaration order, unpadded.
pub const HEADER_LEN: usize = 20;

/// A Win32 `POINT`: two `LONG`s, so two signed 32-bit integers.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: i32,
    /// Vertical coordinate.
    pub y: i32,
}

impl Point {
    /// `(0, 0)` — what a clipboard copy (as opposed to a drop) carries.
    pub const ORIGIN: Self = Self { x: 0, y: 0 };

    /// Construct a point.
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// One path out of the array, still in the encoding it arrived in.
///
/// Kept as an enum rather than normalised to `str` because the ANSI case
/// genuinely cannot be decoded without out-of-band knowledge, and silently
/// guessing Windows-1252 would corrupt every non-Latin path it touched.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Path<'a> {
    /// UTF-16LE bytes, terminating NUL already stripped (`fWide != 0`).
    Wide(&'a [u8]),
    /// Raw bytes in the *source machine's* ANSI codepage (`fWide == 0`).
    ///
    /// The codepage is not in the payload. Decoding is deliberately the
    /// caller's problem — `chars_with` and `to_string_with`, behind the
    /// `codepage` feature, are how a caller that knows it says so.
    Ansi(&'a [u8]),
}

impl<'a> Path<'a> {
    /// The path bytes, whatever their encoding.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        match *self {
            Self::Wide(b) | Self::Ansi(b) => b,
        }
    }

    /// `true` for [`Path::Wide`].
    #[must_use]
    pub const fn is_wide(&self) -> bool {
        matches!(self, Self::Wide(_))
    }

    /// Decode a wide path a `char` at a time; `None` for an ANSI path.
    ///
    /// The iterator yields `Err` on a lone surrogate rather than substituting
    /// U+FFFD, because a path that does not round-trip is worth knowing about
    /// before you try to open it.
    #[must_use]
    pub const fn chars(&self) -> Option<Utf16Le<'a>> {
        match *self {
            Self::Wide(b) => Some(Utf16Le::new(b)),
            Self::Ansi(_) => None,
        }
    }

    /// Decode a wide path, substituting U+FFFD for malformed sequences.
    ///
    /// `None` for [`Path::Ansi`]: there is no codepage to decode it with.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn to_string_lossy(&self) -> Option<alloc::string::String> {
        match *self {
            Self::Wide(b) => Some(rclip_core::utf16::decode_utf16le_lossy(b)),
            Self::Ansi(_) => None,
        }
    }
}

/// A parsed `CF_HDROP` payload.
///
/// Borrows the input; parsing allocates nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropFiles<'a> {
    point: Point,
    non_client: bool,
    wide: bool,
    list_offset: usize,
    /// The path array with its final terminating NUL unit removed, so the
    /// iterator never has to special-case the end.
    list: &'a [u8],
}

impl<'a> DropFiles<'a> {
    /// Parse a `CF_HDROP` payload.
    ///
    /// # Errors
    ///
    /// - [`ErrorKind::UnexpectedEof`] if the header is short, or if the path
    ///   array runs to the end of the buffer without its terminator.
    /// - [`ErrorKind::BadOffset`] if `pFiles` points outside the buffer or back
    ///   into the header.
    pub fn parse(buf: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(buf);

        let p_files = r.u32_le()?;
        let point = Point::new(r.i32_le()?, r.i32_le()?);
        // fNC and fWide are Win32 `BOOL`, i.e. `int`. Any nonzero value is
        // TRUE; do not test against 1, real sources have been seen writing
        // -1 and 0xFFFFFFFF.
        let non_client = r.i32_le()? != 0;
        let wide = r.i32_le()? != 0;
        debug_assert_eq!(r.pos(), HEADER_LEN);

        // `pFiles` is an offset from the start of the DROPFILES struct, which
        // is the start of this buffer — not from the end of the header, and
        // not from the cursor. It is 20 in practice but a producer is free to
        // leave a gap, so honour whatever it says.
        let list_offset =
            usize::try_from(p_files).map_err(|_| Error::new(ErrorKind::TooLarge, 0))?;
        if list_offset < HEADER_LEN {
            // Below 20 the "path array" aliases the header. Windows would
            // happily read the DWORDs back as text; refuse instead.
            return Err(Error::new(ErrorKind::BadOffset, 0));
        }
        // Fails with BadOffset if it points past the end of the buffer.
        r.seek(list_offset)?;

        // TODO(phase-1): decide, against real captures, whether to accept an
        // array whose final NUL was never written. Phase 0 refuses: there is no
        // way to tell a sloppy producer from a truncated buffer, and guessing
        // the former is how you read a partial path as a whole one.
        //
        // Walk the array to find its terminator before handing anything out,
        // so that the iterator below can be infallible. The array ends at the
        // first *empty* string: `a\0b\0\0` is two paths, and the trailing NUL
        // of the last path plus the array terminator is the double NUL the
        // format is named for. Getting this boundary wrong by one unit is the
        // classic CF_HDROP bug in both directions.
        let mut scan = r.clone();
        loop {
            let entry = if wide {
                scan.utf16_nul_bytes()?
            } else {
                scan.cstr_bytes()?
            };
            if entry.is_empty() {
                break;
            }
        }
        let terminator_len = if wide { 2 } else { 1 };
        let list_end = scan.pos() - terminator_len;
        let list = r.slice_at(list_offset, list_end - list_offset)?;

        Ok(Self {
            point,
            non_client,
            wide,
            list_offset,
            list,
        })
    }

    /// The drop point. Screen coordinates when [`Self::is_non_client`],
    /// client coordinates otherwise. `(0, 0)` for a plain clipboard copy.
    #[must_use]
    pub const fn point(&self) -> Point {
        self.point
    }

    /// `fNC`: the drop point is in a window's nonclient area, so [`Self::point`]
    /// is in screen rather than client coordinates.
    #[must_use]
    pub const fn is_non_client(&self) -> bool {
        self.non_client
    }

    /// `fWide`: the path array is UTF-16LE rather than system ANSI.
    #[must_use]
    pub const fn is_wide(&self) -> bool {
        self.wide
    }

    /// The value `pFiles` held, i.e. where the path array actually started.
    #[must_use]
    pub const fn list_offset(&self) -> usize {
        self.list_offset
    }

    /// The path array as it sits in the buffer, minus the array terminator.
    ///
    /// Each path in here is still individually NUL-terminated.
    #[must_use]
    pub const fn raw_list(&self) -> &'a [u8] {
        self.list
    }

    /// Iterate the paths. Borrows; allocates nothing.
    #[must_use]
    pub const fn paths(&self) -> Paths<'a> {
        Paths {
            rest: self.list,
            wide: self.wide,
        }
    }

    /// Number of paths. `O(n)` — it walks the array.
    #[must_use]
    pub fn count(&self) -> usize {
        self.paths().count()
    }

    /// `true` if the payload carries no paths at all.
    ///
    /// A well-formed empty list is legal and is exactly one NUL unit.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Re-serialize to canonical form: `pFiles == 20`, no gap before the array.
    ///
    /// Byte-identical to the input when the input was already canonical, which
    /// is the round-trip property the tests assert. A payload that carried a
    /// gap between header and array comes back without it.
    #[cfg(feature = "alloc")]
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let terminator_len = if self.wide { 2 } else { 1 };
        let mut out = Vec::with_capacity(HEADER_LEN + self.list.len() + terminator_len);
        write_header(&mut out, self.point, self.non_client, self.wide);
        out.extend_from_slice(self.list);
        out.extend_from_slice(&[0u8; 2][..terminator_len]);
        out
    }
}

/// Iterator over the paths of a [`DropFiles`].
///
/// Infallible: [`DropFiles::parse`] already proved the array is terminated, so
/// there is no failure left for the iterator to report.
#[derive(Debug, Clone)]
pub struct Paths<'a> {
    rest: &'a [u8],
    wide: bool,
}

impl<'a> Iterator for Paths<'a> {
    type Item = Path<'a>;

    fn next(&mut self) -> Option<Path<'a>> {
        if self.rest.is_empty() {
            return None;
        }
        // Every entry still in `rest` is NUL-terminated: parse() located the
        // array terminator and cut it off, so there is no unterminated tail to
        // fall off the end of. Both `unwrap_or`s below are consequently
        // unreachable, and exist so that a future change to parse() degrades
        // into a short read rather than a panic.
        let (unit, end) = if self.wide {
            (2, wide_nul_pos(self.rest).unwrap_or(self.rest.len()))
        } else {
            (
                1,
                self.rest
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(self.rest.len()),
            )
        };
        let (out, tail) = self.rest.split_at(end);
        self.rest = tail.get(unit..).unwrap_or(&[]);
        Some(if self.wide {
            Path::Wide(out)
        } else {
            Path::Ansi(out)
        })
    }
}

/// Byte offset of the first UTF-16LE NUL unit, counting in units from 0.
fn wide_nul_pos(bytes: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == 0 && bytes[i + 1] == 0 {
            return Some(i);
        }
        i += 2;
    }
    None
}

#[cfg(feature = "alloc")]
fn write_header(out: &mut Vec<u8>, point: Point, non_client: bool, wide: bool) {
    out.extend_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    out.extend_from_slice(&point.x.to_le_bytes());
    out.extend_from_slice(&point.y.to_le_bytes());
    // Win32 TRUE is 1. Sources that write -1 parse fine, but emitting 1 is
    // what every Windows component does and what a byte-comparing test wants.
    out.extend_from_slice(&i32::from(non_client).to_le_bytes());
    out.extend_from_slice(&i32::from(wide).to_le_bytes());
}

/// Builds a `CF_HDROP` payload.
///
/// This half of the crate is what lets an application *offer* files — drag a
/// selection into Explorer, or put it on the clipboard for Ctrl+V. The builder
/// owns a growing buffer, so it lives behind the `alloc` feature.
///
/// ```
/// use rclip_dropfiles::{Builder, DropFiles, Point};
///
/// # fn main() -> Result<(), rclip_core::Error> {
/// let mut b = Builder::wide().at(Point::new(12, 34));
/// b.push_str("C:\\a.txt")?;
/// b.push_str("C:\\b.txt")?;
/// let bytes = b.finish();
///
/// let parsed = DropFiles::parse(&bytes)?;
/// assert_eq!(parsed.count(), 2);
/// assert_eq!(parsed.point(), Point::new(12, 34));
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "alloc")]
#[derive(Debug, Clone)]
pub struct Builder {
    point: Point,
    non_client: bool,
    wide: bool,
    list: Vec<u8>,
}

#[cfg(feature = "alloc")]
impl Builder {
    /// A builder for a UTF-16LE list. What any modern producer should emit.
    #[must_use]
    pub const fn wide() -> Self {
        Self {
            point: Point::ORIGIN,
            non_client: false,
            wide: true,
            list: Vec::new(),
        }
    }

    /// A builder for a system-ANSI list. Only for talking to software that
    /// cannot read `fWide == 1`; you supply the already-encoded bytes.
    #[must_use]
    pub const fn ansi() -> Self {
        Self {
            point: Point::ORIGIN,
            non_client: false,
            wide: false,
            list: Vec::new(),
        }
    }

    /// Set the drop point. Leave it at [`Point::ORIGIN`] for a clipboard copy.
    #[must_use]
    pub fn at(mut self, point: Point) -> Self {
        self.point = point;
        self
    }

    /// Set `fNC`, declaring the drop point to be in screen coordinates.
    #[must_use]
    pub fn non_client(mut self, yes: bool) -> Self {
        self.non_client = yes;
        self
    }

    /// Append a UTF-16 path from a Rust string.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Unsupported`] on an ANSI builder — a `str` would have to be
    /// transcoded to a codepage this crate does not implement.
    /// [`ErrorKind::Malformed`] if `path` contains a NUL, which would silently
    /// truncate the entry and shift every path after it.
    pub fn push_str(&mut self, path: &str) -> Result<()> {
        if !self.wide {
            return Err(Error::new(ErrorKind::Unsupported, self.list.len()));
        }
        if path.contains('\0') {
            return Err(Error::new(ErrorKind::Malformed, self.list.len()));
        }
        for unit in path.encode_utf16() {
            self.list.extend_from_slice(&unit.to_le_bytes());
        }
        self.list.extend_from_slice(&[0, 0]);
        Ok(())
    }

    /// Append a path in the form the parser hands out, so a payload can be
    /// taken apart and put back together.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Unsupported`] if the variant does not match the builder's
    /// `fWide`; [`ErrorKind::BadLength`] for an odd-length wide path;
    /// [`ErrorKind::Malformed`] for an embedded terminator.
    pub fn push(&mut self, path: Path<'_>) -> Result<()> {
        let at = self.list.len();
        match (path, self.wide) {
            (Path::Wide(bytes), true) => {
                if bytes.len() % 2 != 0 {
                    return Err(Error::new(ErrorKind::BadLength, at));
                }
                if wide_nul_pos(bytes).is_some() {
                    return Err(Error::new(ErrorKind::Malformed, at));
                }
                self.list.extend_from_slice(bytes);
                self.list.extend_from_slice(&[0, 0]);
            }
            (Path::Ansi(bytes), false) => {
                if bytes.contains(&0) {
                    return Err(Error::new(ErrorKind::Malformed, at));
                }
                self.list.extend_from_slice(bytes);
                self.list.push(0);
            }
            _ => return Err(Error::new(ErrorKind::Unsupported, at)),
        }
        Ok(())
    }

    /// Emit the payload.
    ///
    /// An empty builder yields a header plus a single NUL unit: that is a
    /// well-formed list of zero files, not an error.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        let terminator_len = if self.wide { 2 } else { 1 };
        let mut out = Vec::with_capacity(HEADER_LEN + self.list.len() + terminator_len);
        write_header(&mut out, self.point, self.non_client, self.wide);
        out.extend_from_slice(&self.list);
        out.extend_from_slice(&[0u8; 2][..terminator_len]);
        out
    }
}

#[cfg(feature = "alloc")]
impl Default for Builder {
    fn default() -> Self {
        Self::wide()
    }
}

/// Build a wide `CF_HDROP` payload from paths, dropped at the origin.
///
/// The one-liner for "put these files on the clipboard".
///
/// # Errors
///
/// [`ErrorKind::Malformed`] if any path contains a NUL.
#[cfg(feature = "alloc")]
pub fn to_bytes<'s, I>(paths: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = &'s str>,
{
    let mut b = Builder::wide();
    for p in paths {
        b.push_str(p)?;
    }
    Ok(b.finish())
}

/// Decoding and encoding the `fWide == 0` path array with a named code page.
///
/// The system ANSI code page is a property of the machine that wrote the
/// payload and is not in the payload. Everything here therefore takes the
/// encoding as a parameter; nothing in this module guesses.
#[cfg(feature = "codepage")]
mod with_codepage {
    #[cfg(feature = "alloc")]
    extern crate alloc;

    use rclip_codepage::{Decoder, Encoding};
    use rclip_core::{Error, ErrorKind, Result, Utf16Le};

    use super::Path;

    impl<'a> Path<'a> {
        /// Decode a path a `char` at a time, naming the code page an
        /// [`Path::Ansi`] entry is in.
        ///
        /// Unlike [`Path::chars`], this answers for both variants: `enc` is
        /// used for an ANSI path and ignored for a wide one. An undefined byte
        /// yields an error and iteration continues — a single-byte code page
        /// cannot lose sync — whereas a lone surrogate stops the wide iterator,
        /// because after one the rest of the path is not trustworthy.
        #[must_use]
        pub const fn chars_with(&self, enc: Encoding) -> PathChars<'a> {
            match *self {
                Self::Wide(b) => PathChars::Wide(Utf16Le::new(b)),
                Self::Ansi(b) => PathChars::Ansi(enc.decode(b)),
            }
        }

        /// Decode a path with a named code page, failing on anything
        /// undecodable.
        ///
        /// # Errors
        ///
        /// [`ErrorKind::Malformed`] at a byte the code page leaves undefined,
        /// [`ErrorKind::InvalidUtf16`] at a lone surrogate in a wide path.
        /// A path that does not round-trip is worth knowing about before you
        /// try to open it, which is why this exists alongside the lossy form.
        #[cfg(feature = "alloc")]
        pub fn to_string_with(&self, enc: Encoding) -> Result<alloc::string::String> {
            let mut out = alloc::string::String::new();
            for c in self.chars_with(enc) {
                out.push(c?);
            }
            Ok(out)
        }

        /// Decode a path with a named code page, substituting U+FFFD.
        #[cfg(feature = "alloc")]
        #[must_use]
        pub fn to_string_lossy_with(&self, enc: Encoding) -> alloc::string::String {
            let mut out = alloc::string::String::new();
            for c in self.chars_with(enc) {
                out.push(c.unwrap_or('\u{FFFD}'));
            }
            out
        }
    }

    /// Iterator over the `char`s of a [`Path`] decoded with a named code page.
    /// Returned by [`Path::chars_with`].
    #[derive(Debug, Clone)]
    pub enum PathChars<'a> {
        #[doc(hidden)]
        Wide(Utf16Le<'a>),
        #[doc(hidden)]
        Ansi(Decoder<'a>),
    }

    impl Iterator for PathChars<'_> {
        type Item = Result<char>;

        fn next(&mut self) -> Option<Self::Item> {
            match self {
                Self::Wide(it) => it.next(),
                Self::Ansi(it) => it.next(),
            }
        }
    }

    impl core::iter::FusedIterator for PathChars<'_> {}

    #[cfg(feature = "alloc")]
    impl super::Builder {
        /// Append a path to an ANSI builder, encoding it with `enc`.
        ///
        /// The counterpart to [`super::Builder::push_str`], which refuses on an
        /// ANSI builder precisely because it has no code page to encode with.
        ///
        /// # Errors
        ///
        /// [`ErrorKind::Unsupported`] on a *wide* builder — use `push_str`
        /// there, and carrying an encoding on the wide path would only invite
        /// someone to pass the wrong one. [`ErrorKind::Unsupported`] also when
        /// `path` holds a character `enc` cannot represent, at that character's
        /// byte offset within `path`: silently dropping it would produce a
        /// payload naming a file that does not exist.
        /// [`ErrorKind::Malformed`] if `path` contains a NUL, which would
        /// truncate the entry and shift every path after it.
        pub fn push_str_encoded(&mut self, path: &str, enc: Encoding) -> Result<()> {
            if self.wide {
                return Err(Error::new(ErrorKind::Unsupported, self.list.len()));
            }
            // Encode before appending anything: a character the code page
            // cannot represent must leave the builder untouched, or the caller
            // handling the error still ships a truncated path.
            let bytes = enc.encode_from_str(path)?;
            self.push(Path::Ansi(&bytes))
        }
    }
}

#[cfg(feature = "codepage")]
pub use with_codepage::PathChars;

/// Re-exported so a caller can name a code page without adding `rclip-codepage`
/// to its own manifest.
#[cfg(feature = "codepage")]
pub use rclip_codepage::Encoding;

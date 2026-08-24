//! Writing `CF_HTML`.
//!
//! # Why this is the hard half
//!
//! Every offset in a `CF_HTML` header is a byte position *in the buffer the
//! header is part of*. Writing `StartHTML` therefore requires knowing how long
//! the header is, which requires knowing how many digits `StartHTML` has. The
//! naive fix — write the offsets, measure, write them again, repeat — either
//! oscillates or converges by luck, and every implementation that tries it has
//! an off-by-one somewhere.
//!
//! The fix the spec itself suggests is to make the field width constant:
//!
//! > programs sniffing the HTML for the offsets could write ten (10) zeros to
//! > its output buffer for each keyword […] Later, when the exact `StartHTML`
//! > offset is known (say, 71), the program can overwrite the rightmost zeroes
//! > with "71" in the buffer
//!
//! So this module emits `0000000000` for every offset, remembers where each
//! field starts, appends the whole body, and then overwrites the ten digits in
//! place. Nothing moves, nothing is recomputed, and there is no second pass.
//! Ten digits addresses just under 10 GB, which is four orders of magnitude
//! more than any clipboard payload.
//!
//! The one case that is *not* a placeholder is `-1`. A blob with no context
//! knows that value before it starts, and `-1` does not fit the fixed-width
//! zero-padded shape, so those two lines are written out literally.

use alloc::vec::Vec;
use core::ops::Range;

use rclip_core::{Error, ErrorKind, Result};

use crate::marker;
use crate::{Version, END_FRAGMENT_COMMENT, START_FRAGMENT_COMMENT};

/// Width of a back-patched offset field, and the reason the patch is safe:
/// the field is the same size before and after, so nothing downstream shifts.
const FIELD_WIDTH: usize = 10;

/// Largest value a [`FIELD_WIDTH`]-digit field can hold.
const FIELD_MAX: usize = 9_999_999_999;

/// The default document wrapped around a fragment when no context is given.
const DEFAULT_CONTEXT: (&str, &str) = ("<html><body>", "</body></html>");

/// Builds a `CF_HTML` blob around a fragment of HTML.
///
/// ```
/// # use rclip_cf_html::{parse, CfHtmlBuilder};
/// let blob = CfHtmlBuilder::new("<b>hi</b>")
///     .source_url("https://example.com/page")
///     .build()
///     .unwrap();
/// let back = parse(&blob).unwrap();
/// assert_eq!(back.fragment, "<b>hi</b>");
/// assert_eq!(back.source_url, Some("https://example.com/page"));
/// ```
#[derive(Debug, Clone)]
pub struct CfHtmlBuilder<'a> {
    fragment: &'a str,
    context: Option<(&'a str, &'a str)>,
    version: Version<'a>,
    source_url: Option<&'a str>,
    selection: Option<Range<usize>>,
}

impl<'a> CfHtmlBuilder<'a> {
    /// Start a blob whose fragment is `fragment`.
    ///
    /// `fragment` is the inner HTML — the marker comments are added by
    /// [`build`](Self::build) and must not be in it.
    #[must_use]
    pub fn new(fragment: &'a str) -> Self {
        Self {
            fragment,
            context: Some(DEFAULT_CONTEXT),
            version: Version::V0_9,
            source_url: None,
            selection: None,
        }
    }

    /// Set the `Version:` value.
    ///
    /// The default is `0.9`, not the newer `1.0`. `0.9` is what Chrome,
    /// Firefox and Word all still write, so every consumer in existence
    /// accepts it, whereas a consumer predating Windows 10 20H2 has never seen
    /// `1.0`. Nothing in the format changed between the two.
    #[must_use]
    pub fn version(mut self, version: Version<'a>) -> Self {
        self.version = version;
        self
    }

    /// Wrap the fragment in `before` … `after` and declare that as the context.
    ///
    /// The marker comments go *inside* this pair, which is where the spec puts
    /// them: the context is a complete document that contains the fragment.
    #[must_use]
    pub fn context(mut self, before: &'a str, after: &'a str) -> Self {
        self.context = Some((before, after));
        self
    }

    /// Emit `StartHTML:-1` / `EndHTML:-1` and no surrounding document.
    ///
    /// The spec allows this and says the fragment alone carries enough for a
    /// basic paste. It costs the consumer any `<base href>` and any inherited
    /// styling, so prefer [`context`](Self::context) when you have one.
    #[must_use]
    pub fn no_context(mut self) -> Self {
        self.context = None;
        self
    }

    /// Set the `SourceURL` header, so a consumer can resolve relative links.
    #[must_use]
    pub fn source_url(mut self, url: &'a str) -> Self {
        self.source_url = Some(url);
        self
    }

    /// Mark a byte range *within the fragment* as the user's exact selection.
    ///
    /// The range is relative to `fragment`, not to the finished blob; the
    /// absolute `StartSelection`/`EndSelection` offsets are computed during
    /// [`build`](Self::build). Taking it relative is what makes the value
    /// meaningful before the blob exists.
    #[must_use]
    pub fn selection(mut self, range: Range<usize>) -> Self {
        self.selection = Some(range);
        self
    }

    /// Serialize.
    ///
    /// # Errors
    ///
    /// - [`ErrorKind::Malformed`] if the fragment or the context already
    ///   contains a `StartFragment`/`EndFragment` marker comment — that would
    ///   move the fragment boundary a parser finds, silently truncating or
    ///   extending what the next application pastes.
    /// - [`ErrorKind::Malformed`] if the version or the source URL contains a
    ///   line break, which would inject a header line, or if the version
    ///   contains a colon, which would split into a bogus key.
    /// - [`ErrorKind::BadOffset`] if the selection range runs backwards, ends
    ///   past the fragment, or does not land on UTF-8 character boundaries.
    /// - [`ErrorKind::TooLarge`] if the finished blob would exceed 9,999,999,999
    ///   bytes and so could not be addressed by a ten-digit offset.
    pub fn build(&self) -> Result<Vec<u8>> {
        self.validate()?;

        let mut buf: Vec<u8> = Vec::new();

        buf.extend_from_slice(b"Version:");
        buf.extend_from_slice(self.version.as_str().as_bytes());
        buf.extend_from_slice(b"\r\n");

        let has_context = self.context.is_some();
        // `-1` is known up front and is not ten zero-padded digits, so it is
        // written literally instead of being reserved and patched.
        let start_html_field = reserve(&mut buf, "StartHTML", has_context);
        let end_html_field = reserve(&mut buf, "EndHTML", has_context);
        let start_fragment_field = reserve(&mut buf, "StartFragment", true);
        let end_fragment_field = reserve(&mut buf, "EndFragment", true);
        let (start_selection_field, end_selection_field) = if self.selection.is_some() {
            (
                reserve(&mut buf, "StartSelection", true),
                reserve(&mut buf, "EndSelection", true),
            )
        } else {
            (None, None)
        };
        if let Some(url) = self.source_url {
            buf.extend_from_slice(b"SourceURL:");
            buf.extend_from_slice(url.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }

        let start_html = buf.len();
        if let Some((before, _)) = self.context {
            buf.extend_from_slice(before.as_bytes());
        }
        buf.extend_from_slice(START_FRAGMENT_COMMENT.as_bytes());
        let start_fragment = buf.len();
        buf.extend_from_slice(self.fragment.as_bytes());
        let end_fragment = buf.len();
        buf.extend_from_slice(END_FRAGMENT_COMMENT.as_bytes());
        if let Some((_, after)) = self.context {
            buf.extend_from_slice(after.as_bytes());
        }
        let end_html = buf.len();

        if end_html > FIELD_MAX {
            return Err(Error::new(ErrorKind::TooLarge, end_html));
        }

        patch(&mut buf, start_html_field, start_html)?;
        patch(&mut buf, end_html_field, end_html)?;
        patch(&mut buf, start_fragment_field, start_fragment)?;
        patch(&mut buf, end_fragment_field, end_fragment)?;
        if let Some(sel) = &self.selection {
            patch(&mut buf, start_selection_field, start_fragment + sel.start)?;
            patch(&mut buf, end_selection_field, start_fragment + sel.end)?;
        }

        Ok(buf)
    }

    fn validate(&self) -> Result<()> {
        let version = self.version.as_str();
        if version.is_empty()
            || version
                .bytes()
                .any(|b| b == b'\r' || b == b'\n' || b == b':')
        {
            return Err(Error::new(ErrorKind::Malformed, 0));
        }
        if let Some(url) = self.source_url {
            if url.bytes().any(|b| b == b'\r' || b == b'\n') {
                return Err(Error::new(ErrorKind::Malformed, 0));
            }
        }

        // A marker comment anywhere in the payload other than where we put it
        // relocates the fragment boundary for every parser downstream — ours
        // included, since ours believes the comments over the numbers.
        for text in [
            Some(self.fragment),
            self.context.map(|c| c.0),
            self.context.map(|c| c.1),
        ]
        .into_iter()
        .flatten()
        {
            for name in [marker::START, marker::END] {
                if let Some((at, _)) = marker::find(text.as_bytes(), name) {
                    return Err(Error::new(ErrorKind::Malformed, at));
                }
            }
        }

        if let Some(sel) = &self.selection {
            if sel.start > sel.end || sel.end > self.fragment.len() {
                return Err(Error::new(ErrorKind::BadOffset, sel.end));
            }
            // Slicing a `&str` off a boundary panics. A selection range comes
            // from a caller that measured a rendered document, so it can land
            // mid-codepoint; refuse it here rather than let a parser downstream
            // hand back an `InvalidUtf8` for markup that was fine.
            if !self.fragment.is_char_boundary(sel.start) {
                return Err(Error::new(ErrorKind::BadOffset, sel.start));
            }
            if !self.fragment.is_char_boundary(sel.end) {
                return Err(Error::new(ErrorKind::BadOffset, sel.end));
            }
        }

        Ok(())
    }
}

/// Write `key:` followed either by a ten-digit placeholder (returning where the
/// digits start, for [`patch`]) or by a literal `-1` (returning `None`).
fn reserve(buf: &mut Vec<u8>, key: &str, placeholder: bool) -> Option<usize> {
    buf.extend_from_slice(key.as_bytes());
    buf.push(b':');
    let field = if placeholder {
        let at = buf.len();
        buf.extend_from_slice(&[b'0'; FIELD_WIDTH]);
        Some(at)
    } else {
        buf.extend_from_slice(b"-1");
        None
    };
    buf.extend_from_slice(b"\r\n");
    field
}

/// Overwrite a reserved field with `value`, right-aligned and zero-padded.
///
/// `field` is `None` for a `-1` line, which has nothing to patch.
fn patch(buf: &mut [u8], field: Option<usize>, value: usize) -> Result<()> {
    let Some(at) = field else { return Ok(()) };
    if value > FIELD_MAX {
        return Err(Error::new(ErrorKind::TooLarge, value));
    }
    let slot = buf
        .get_mut(at..at + FIELD_WIDTH)
        .ok_or_else(|| Error::new(ErrorKind::BadOffset, at))?;
    let mut v = value;
    for byte in slot.iter_mut().rev() {
        *byte = b'0' + (v % 10) as u8;
        v /= 10;
    }
    Ok(())
}

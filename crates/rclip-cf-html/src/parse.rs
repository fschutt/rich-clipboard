//! Resolving a header plus a body into borrowed `&str` views.

use rclip_core::{Error, ErrorKind, Reader, Result};

use crate::header::{parse_header, Header, Offset, Version};
use crate::marker;

/// A parsed `CF_HTML` payload.
///
/// Every field borrows from the buffer handed to [`parse`]; nothing is copied
/// and nothing is allocated.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CfHtml<'a> {
    /// The `Version:` value. `0.9` and `1.0` are the two defined ones; see
    /// [`Version`] for why others are accepted.
    pub version: Version<'a>,
    /// The surrounding document, when the producer supplied one.
    ///
    /// `None` means the header said `StartHTML:-1` / `EndHTML:-1`, or omitted
    /// them: a fragment with no context. Pasting then has nothing to inherit
    /// styles or a `<base href>` from.
    pub context: Option<&'a str>,
    /// The fragment text — what the user actually copied, without the marker
    /// comments around it. This is the field a paste handler wants.
    pub fragment: &'a str,
    /// The user's exact selection, when the producer supplied one.
    ///
    /// This is a raw text range and is *not* balanced HTML: the spec is
    /// explicit that a selection can start in one element and end in an
    /// ancestor, so it may be a fragment of markup that does not parse on its
    /// own. Use it to know what was highlighted, not to build a document.
    pub selection: Option<&'a str>,
    /// The page the fragment was copied from, from the `SourceURL` header.
    ///
    /// Used to resolve relative URLs in the fragment. Absent from the current
    /// Win32 grammar, but emitted by Internet Explorer, Edge, Chrome and Word.
    pub source_url: Option<&'a str>,
}

/// Where the fragment boundaries came from, and whether the two sources agreed.
///
/// A caller that is auditing clipboard producers, or deciding how much to trust
/// a payload, wants this. A caller that just wants to paste can ignore it.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum FragmentSource {
    /// The marker comments and the byte offsets pointed at the same bytes.
    Agreed,
    /// Both were present and they disagreed. The comments won.
    ///
    /// This is not a corner case: Microsoft's own documented MSHTML example
    /// disagrees with itself this way.
    CommentsOverrodeOffsets,
    /// Only the marker comments were usable — `StartFragment`/`EndFragment`
    /// were missing or negative.
    CommentsOnly,
    /// Only the byte offsets were usable — the marker comments were missing.
    /// This is the one case where a wrong number silently yields wrong text,
    /// because there is nothing to check it against.
    OffsetsOnly,
}

/// A parse, with the evidence it was based on.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Parsed<'a> {
    /// The resolved content.
    pub content: CfHtml<'a>,
    /// The header exactly as written, uncorrected and unclamped. Compare it
    /// against `content` to see what the producer got wrong.
    pub header: Header<'a>,
    /// Which of the two redundant fragment delimiters was believed.
    pub fragment_source: FragmentSource,
    /// The selection's byte range within [`CfHtml::fragment`], when it lies
    /// inside the fragment — which it normally does, but the format does not
    /// require it.
    ///
    /// Present so a caller can re-serialize a payload without having to
    /// rediscover where inside the fragment the selection was.
    pub selection_in_fragment: Option<(usize, usize)>,
}

/// Parse a `CF_HTML` blob.
///
/// The bytes are the payload of the `"HTML Format"` clipboard item, starting at
/// the `V` of `Version:`. Do not strip anything first — the offsets inside are
/// relative to this exact buffer.
///
/// A leading UTF-8 BOM is tolerated, and its three bytes are counted as part of
/// the offsets — see [`Header::bom_len`].
///
/// A repeated keyword takes its **first** value, and [`Header::duplicate_keys`]
/// records that it happened. The spec floats multiple
/// `StartFragment`/`EndFragment` pairs as a future extension for non-contiguous
/// selections; nothing has ever emitted one, so a repeat today is a producer bug
/// or a deliberate ambiguity between two readers, and first-wins is the reading
/// that cannot be changed by appending to the header.
///
/// # Errors
///
/// - [`ErrorKind::BadMagic`] if there is no `Version:` line, which is how a
///   buffer that is not `CF_HTML` at all is rejected.
/// - [`ErrorKind::BadOffset`] if an offset points past the end of the buffer.
/// - [`ErrorKind::Malformed`] if an offset is not a number, if a range runs
///   backwards, if only one half of the selection pair is present, or if the
///   fragment cannot be located at all.
/// - [`ErrorKind::InvalidUtf8`] if the body is not UTF-8.
pub fn parse(blob: &[u8]) -> Result<CfHtml<'_>> {
    parse_detailed(blob).map(|p| p.content)
}

/// Parse a `CF_HTML` blob, keeping the raw header and the provenance of the
/// fragment boundaries.
///
/// Same failure modes as [`parse`].
pub fn parse_detailed(blob: &[u8]) -> Result<Parsed<'_>> {
    // A leading UTF-8 BOM is skipped by `parse_header` for the purpose of
    // reading `Key:Value` lines, and counted for the purpose of everything
    // else: the reader below is over the *whole* blob, so `header_len` includes
    // the mark and every offset resolves against the buffer as it arrived. See
    // `Header::bom_len`.
    let mut r = Reader::new(blob);
    let header = parse_header(&mut r)?;
    let len = blob.len();
    let body_start = header.header_len;

    let context = match (header.start_html, header.end_html) {
        (Offset::At(s), Offset::At(e)) => Some(resolve(&r, s, e, body_start)?),
        // A `StartHTML` with no `EndHTML` is taken to run to the end of the
        // blob. Half a context is still more useful than none, and there is
        // only one sensible place for it to end.
        (Offset::At(s), Offset::Absent) => Some(resolve(&r, s, len, body_start)?),
        // `-1`, or no `StartHTML` at all: fragment only.
        _ => None,
    };

    let body = r.tail_at(body_start)?;
    let from_comments = marker::find_fragment(body)
        .map(|(s, e)| (body_start.saturating_add(s), body_start.saturating_add(e)));
    let from_offsets = match (header.start_fragment, header.end_fragment) {
        (Offset::At(s), Offset::At(e)) => Some((s, e)),
        _ => None,
    };

    let (fragment_range, fragment_source) = match (from_comments, from_offsets) {
        // The comments win whenever they exist. They are markup the producer
        // physically inserted into the text it also transformed, so they move
        // with the content; the byte counts are computed separately and drift.
        (Some(c), Some(o)) if c == o => (c, FragmentSource::Agreed),
        (Some(c), Some(_)) => (c, FragmentSource::CommentsOverrodeOffsets),
        (Some(c), None) => (c, FragmentSource::CommentsOnly),
        (None, Some(o)) => (o, FragmentSource::OffsetsOnly),
        (None, None) => {
            return Err(Error::new(ErrorKind::Malformed, body_start));
        }
    };
    let fragment = resolve(&r, fragment_range.0, fragment_range.1, body_start)?;

    let (selection, selection_in_fragment) = match (header.start_selection, header.end_selection) {
        (Offset::At(s), Offset::At(e)) => {
            let text = resolve(&r, s, e, body_start)?;
            let (fs, fe) = fragment_range;
            // Measure against the *clamped* fragment start, exactly as
            // `resolve` computed it. A `StartFragment` that points inside the
            // header is raised to the end of the header (see `resolve`), so
            // `fragment` can be shorter than `fe - fs`; subtracting the
            // unclamped `fs` here reported a range running past the end of the
            // very string it is documented to index, and `&fragment[a..b]` --
            // the one thing this field exists for -- then panicked on a
            // caller-visible slice of attacker-controlled clipboard bytes.
            let fs = fs.max(body_start).min(fe);
            let inside = if s >= fs && e <= fe {
                Some((s - fs, e - fs))
            } else {
                None
            };
            (Some(text), inside)
        }
        // Both keywords present but at least one negative: the producer said
        // "no selection" the same way it says "no context". Both absent: same
        // outcome. The half-present case was already rejected in the header.
        _ => (None, None),
    };

    Ok(Parsed {
        content: CfHtml {
            version: header.version,
            context,
            fragment,
            selection,
            source_url: header.source_url,
        },
        header,
        fragment_source,
        selection_in_fragment,
    })
}

/// Turn a `[start, end)` pair of blob-absolute offsets into a `&str`.
///
/// `floor` is the end of the header. An offset below it is clamped up rather
/// than rejected: a producer that under-reports `StartHTML` by a byte or two is
/// common (one shipping crate is off by exactly one), and the recovery is
/// unambiguous — the header ends where the header ends. Clamping keeps header
/// bytes from leaking into pasted markup while still returning the content.
///
/// An offset past the *end* of the buffer gets no such courtesy. It means the
/// payload was truncated in transit and the bytes it names do not exist, so
/// there is nothing to recover and guessing would fabricate content.
fn resolve<'a>(r: &Reader<'a>, start: usize, end: usize, floor: usize) -> Result<&'a str> {
    let buf_len = r.len();
    if start > buf_len {
        return Err(Error::new(ErrorKind::BadOffset, start));
    }
    if end > buf_len {
        return Err(Error::new(ErrorKind::BadOffset, end));
    }
    if end < start {
        return Err(Error::new(ErrorKind::Malformed, end));
    }
    let start = start.max(floor).min(end);
    // `slice_at` rather than `&buf[start..end]`: rule 3 of the workspace
    // conventions. Every one of these numbers came off the wire.
    let bytes = r.slice_at(start, end - start)?;
    core::str::from_utf8(bytes).map_err(|e| {
        Error::new(
            ErrorKind::InvalidUtf8,
            start.saturating_add(e.valid_up_to()),
        )
    })
}

//! The `CF_HTML` description header: `Key:Value` lines of ASCII, terminated by
//! the first line that is not shaped like one.
//!
//! Everything here is deliberately lenient about *syntax* and strict about
//! *arithmetic*. Producers get the whitespace, the padding, the line endings
//! and the key set wrong all the time; what they must not be allowed to do is
//! hand us a number that turns into an out-of-bounds slice.

use rclip_core::{Error, ErrorKind, Reader, Result};

/// Longest key this parser will look for a `:` inside.
///
/// Bounds the colon scan so a multi-megabyte body line does not get searched
/// end to end before we conclude it is not a header. The longest real key,
/// `StartSelection`, is 14 bytes.
const MAX_KEY_LEN: usize = 64;

/// Most significant digits an offset value may have after leading zeros are
/// stripped. `usize::MAX` is 20 digits, so anything longer cannot be an offset
/// into a buffer that exists.
const MAX_OFFSET_DIGITS: usize = 20;

/// The `Version:` value.
///
/// Both known versions are accepted and so is anything else: the spec says
/// future revisions may extend the format, and refusing to paste because a
/// producer wrote `Version:1.1` would be a self-inflicted wound. The variant
/// is kept rather than normalized so a re-serialize gives the value back
/// unchanged.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Version<'a> {
    /// `0.9` — the original version, and still what Chrome, Firefox and Word
    /// write today.
    V0_9,
    /// `1.0` — what Windows itself has written since Windows 10 20H2.
    V1_0,
    /// Anything else, verbatim.
    Other(&'a str),
}

impl<'a> Version<'a> {
    /// The value exactly as it appears (or would appear) after `Version:`.
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        match self {
            Self::V0_9 => "0.9",
            Self::V1_0 => "1.0",
            Self::Other(s) => s,
        }
    }

    fn from_bytes(v: &'a [u8], at: usize) -> Result<Self> {
        let s = core::str::from_utf8(v)
            .map_err(|e| Error::new(ErrorKind::InvalidUtf8, at.saturating_add(e.valid_up_to())))?;
        Ok(match s {
            "0.9" => Self::V0_9,
            "1.0" => Self::V1_0,
            other => Self::Other(other),
        })
    }
}

/// One offset field of the description header.
///
/// Three states rather than `Option<usize>`, because "the keyword was missing"
/// and "the keyword said `-1`" mean different things to a caller: the first is
/// a producer that did not emit the field, the second is a producer explicitly
/// saying *there is no context, only a fragment*.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Offset {
    /// The keyword did not appear in the header at all.
    Absent,
    /// The keyword appeared with a negative value — canonically `-1`, which
    /// the spec defines as "no context". Any negative value is folded here;
    /// none of them can be an offset.
    Negative,
    /// A byte offset from the start of the whole blob.
    At(usize),
}

impl Offset {
    /// The byte offset, if this field carried one.
    #[must_use]
    pub const fn at(self) -> Option<usize> {
        match self {
            Self::At(n) => Some(n),
            _ => None,
        }
    }

    /// Whether the keyword appeared, regardless of what it said.
    ///
    /// This is the predicate the `StartSelection`/`EndSelection` both-or-neither
    /// rule is written against.
    #[must_use]
    pub const fn is_present(self) -> bool {
        !matches!(self, Self::Absent)
    }
}

/// The description header, exactly as the producer wrote it.
///
/// Nothing here is corrected or clamped. [`crate::Parsed`] pairs this with the
/// content the parser actually resolved, so a caller that cares can compare the
/// two and see that, say, `start_fragment` disagreed with the marker comments.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Header<'a> {
    /// The `Version:` value.
    pub version: Version<'a>,
    /// `StartHTML` — start of the context.
    pub start_html: Offset,
    /// `EndHTML` — end of the context.
    pub end_html: Offset,
    /// `StartFragment` — start of the fragment text, i.e. the byte just past
    /// the `<!--StartFragment-->` comment.
    pub start_fragment: Offset,
    /// `EndFragment` — end of the fragment text, i.e. the first byte of the
    /// `<!--EndFragment-->` comment.
    pub end_fragment: Offset,
    /// `StartSelection` — start of the user's exact selection. Optional.
    pub start_selection: Offset,
    /// `EndSelection` — end of the user's exact selection. Optional.
    pub end_selection: Offset,
    /// `SourceURL` — the page the fragment was copied from.
    ///
    /// Absent from the current Win32 grammar but documented in the archived
    /// Internet Explorer page and emitted by every mainstream browser, so it is
    /// parsed as a first-class field rather than left as an unknown key.
    pub source_url: Option<&'a str>,
    /// How many bytes the header occupies. The body begins here.
    ///
    /// This is the parser's own answer, independent of `start_html`, and it is
    /// what a lying `StartHTML` gets clamped against.
    pub header_len: usize,
}

/// Split a line off the front of `rest`.
///
/// Returns `(content_len, total_len)`; the difference is the terminator, which
/// may be `\r\n`, `\n`, or a lone `\r`. `None` means there is no terminator
/// left, which ends the header — a header line always has one, because the
/// body follows it.
fn split_line(rest: &[u8]) -> Option<(usize, usize)> {
    let i = rest.iter().position(|&b| b == b'\r' || b == b'\n')?;
    // A lone `\r` is a legal terminator, so `\r` must not be assumed to be half
    // of a `\r\n`. This is also why `str::lines` cannot be used: it does not
    // split on a bare carriage return.
    let total = if rest[i] == b'\r' && rest.get(i + 1) == Some(&b'\n') {
        i + 2
    } else {
        i + 1
    };
    Some((i, total))
}

/// Split `Key:Value`, or return `None` if this line is not a header line.
///
/// The shape test — a non-empty key of ASCII alphanumerics, `-` and `_` — is
/// what stops the header scan at the body. It matters that the test is on the
/// *key* and not merely on the presence of a colon: a body that opens with
/// `<a href="https://…">` has a colon in its first line, and a parser that
/// treats "contains a colon" as "is a header" will swallow it.
fn split_header_line(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let scan = &line[..line.len().min(MAX_KEY_LEN + 1)];
    let i = scan.iter().position(|&b| b == b':')?;
    if i == 0 {
        return None;
    }
    let key = &line[..i];
    if !key
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some((key, &line[i + 1..]))
}

fn key_is(key: &[u8], name: &str) -> bool {
    // Case-insensitive on purpose. The spec fixes the spelling, but the cost of
    // tolerating `STARTHTML` is one instruction and the cost of rejecting it is
    // a paste that silently does nothing.
    key.eq_ignore_ascii_case(name.as_bytes())
}

/// Parse one offset value.
///
/// `at` is the absolute offset of the value in the blob, so a failure points at
/// the digits that caused it.
fn parse_offset(value: &[u8], at: usize) -> Result<Offset> {
    // Trim, because the spec's own "Offset syntax" section writes the example
    // as `StartHTML: 0000000000`, with a space after the colon.
    let v = value.trim_ascii();
    let leading_ws = value.len() - value.trim_ascii_start().len();
    let at = at.saturating_add(leading_ws);

    if v.is_empty() {
        return Err(Error::new(ErrorKind::Malformed, at));
    }
    if v[0] == b'-' {
        let digits = &v[1..];
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return Err(Error::new(ErrorKind::Malformed, at));
        }
        return Ok(Offset::Negative);
    }
    if !v.iter().all(u8::is_ascii_digit) {
        return Err(Error::new(ErrorKind::Malformed, at));
    }

    // Strip leading zeros *without* letting an all-zero field become an empty
    // string. `"0000000000"` is the number zero — it is what a producer writes
    // into its placeholder before it knows the answer, and reading it as a
    // parse failure is a bug that has shipped.
    let mut digits = v;
    while digits.len() > 1 && digits[0] == b'0' {
        digits = &digits[1..];
    }
    if digits.len() > MAX_OFFSET_DIGITS {
        return Err(Error::new(ErrorKind::TooLarge, at));
    }

    let mut n: usize = 0;
    for &d in digits {
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(usize::from(d - b'0')))
            .ok_or_else(|| Error::new(ErrorKind::TooLarge, at))?;
    }
    Ok(Offset::At(n))
}

/// Consume the description header, leaving the reader at the first byte of the
/// body.
pub(crate) fn parse_header<'a>(r: &mut Reader<'a>) -> Result<Header<'a>> {
    let mut h = Header {
        version: Version::Other(""),
        start_html: Offset::Absent,
        end_html: Offset::Absent,
        start_fragment: Offset::Absent,
        end_fragment: Offset::Absent,
        start_selection: Offset::Absent,
        end_selection: Offset::Absent,
        source_url: None,
        header_len: 0,
    };
    let mut version_seen = false;
    let mut selection_key_at = 0usize;

    loop {
        let line_at = r.pos();
        let rest = r.remaining();
        if rest.is_empty() {
            break;
        }
        let (line_len, total) = match split_line(rest) {
            Some(v) => v,
            None => break,
        };
        let (key, value) = match split_header_line(&rest[..line_len]) {
            Some(v) => v,
            None => break,
        };
        let value_at = line_at + key.len() + 1;

        if key_is(key, "Version") {
            h.version = Version::from_bytes(value.trim_ascii(), value_at)?;
            version_seen = true;
        } else if key_is(key, "StartHTML") {
            h.start_html = parse_offset(value, value_at)?;
        } else if key_is(key, "EndHTML") {
            h.end_html = parse_offset(value, value_at)?;
        } else if key_is(key, "StartFragment") {
            // TODO(phase-1): the spec floats "multiple StartFragment and
            // EndFragment pairs [...] to support noncontiguous selection of
            // fragments" as a future extension. Nothing emits them, and there
            // is nowhere in a borrowed, non-allocating return type to put a
            // list. A repeated key currently overwrites: last one wins.
            h.start_fragment = parse_offset(value, value_at)?;
        } else if key_is(key, "EndFragment") {
            h.end_fragment = parse_offset(value, value_at)?;
        } else if key_is(key, "StartSelection") {
            h.start_selection = parse_offset(value, value_at)?;
            selection_key_at = value_at;
        } else if key_is(key, "EndSelection") {
            h.end_selection = parse_offset(value, value_at)?;
            selection_key_at = value_at;
        } else if key_is(key, "SourceURL") {
            let v = value.trim_ascii();
            let leading_ws = value.len() - value.trim_ascii_start().len();
            h.source_url = Some(core::str::from_utf8(v).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidUtf8,
                    value_at + leading_ws + e.valid_up_to(),
                )
            })?);
        }
        // Any other key is skipped in silence. The spec reserves the right to
        // extend the header ("multiple StartFragment/EndFragment pairs could be
        // added later"), so an unknown key is a forward-compatible producer,
        // not a malformed blob.

        r.skip(total)?;

        // Stop as soon as the cursor reaches the declared start of the body.
        // Without this, a fragment whose first line happens to look like
        // `Note: something` would be eaten as an unknown header key. With it,
        // any producer that got `StartHTML` right is immune to that.
        if let Offset::At(s) = h.start_html {
            if r.pos() >= s {
                break;
            }
        }
    }

    h.header_len = r.pos();

    // A `Version:` line is the closest thing CF_HTML has to a magic number.
    // Requiring one is what keeps an arbitrary text blob — or bare HTML handed
    // to the wrong decoder — from being interpreted as a fragment with no
    // context and no offsets.
    if !version_seen {
        return Err(Error::new(ErrorKind::BadMagic, 0));
    }

    // "The StartSelection and EndSelection keywords are optional and must both
    // be omitted." Half a pair means the producer wrote a range whose other end
    // we would have to invent, and inventing it silently mislabels which text
    // the user actually selected.
    match (h.start_selection.is_present(), h.end_selection.is_present()) {
        (true, false) | (false, true) => {
            return Err(Error::new(ErrorKind::Malformed, selection_key_at))
        }
        _ => {}
    }

    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_zero_offset_is_zero_not_an_error() {
        assert_eq!(parse_offset(b"0000000000", 0).unwrap(), Offset::At(0));
    }

    #[test]
    fn arbitrary_left_padding_is_stripped() {
        assert_eq!(parse_offset(b"0000000121", 0).unwrap(), Offset::At(121));
        assert_eq!(parse_offset(b"121", 0).unwrap(), Offset::At(121));
        assert_eq!(
            parse_offset(b"00000000000000000000000121", 0).unwrap(),
            Offset::At(121)
        );
    }

    #[test]
    fn a_space_after_the_colon_is_tolerated() {
        // The spec's own "Offset syntax" prose writes `StartHTML: 0000000000`.
        assert_eq!(parse_offset(b" 0000000071 ", 0).unwrap(), Offset::At(71));
    }

    #[test]
    fn minus_one_is_negative_not_a_huge_number() {
        assert_eq!(parse_offset(b"-1", 0).unwrap(), Offset::Negative);
    }

    #[test]
    fn non_digits_are_malformed_and_carry_the_offset() {
        let err = parse_offset(b"12a4", 40).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Malformed);
        assert_eq!(
            err.offset, 40,
            "the error must point at the value, not the line"
        );
    }

    #[test]
    fn an_offset_wider_than_usize_is_too_large_not_a_wrap() {
        let err = parse_offset(b"99999999999999999999999", 0).unwrap_err();
        assert_eq!(err.kind, ErrorKind::TooLarge);
    }

    #[test]
    fn lone_cr_terminates_a_line() {
        assert_eq!(split_line(b"Version:1.0\rStartHTML:1"), Some((11, 12)));
        assert_eq!(split_line(b"Version:1.0\r\nStartHTML:1"), Some((11, 13)));
        assert_eq!(split_line(b"Version:1.0\nStartHTML:1"), Some((11, 12)));
    }

    #[test]
    fn a_body_line_containing_a_colon_is_not_a_header_line() {
        assert!(split_header_line(br#"<a href="https://example.com/">x</a>"#).is_none());
        assert!(split_header_line(b":leading-colon").is_none());
        assert_eq!(
            split_header_line(b"SourceURL:https://x/"),
            Some((&b"SourceURL"[..], &b"https://x/"[..]))
        );
    }
}

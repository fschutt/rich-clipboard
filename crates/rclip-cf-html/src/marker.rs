//! Finding the `<!--StartFragment-->` / `<!--EndFragment-->` marker comments.
//!
//! These are the *authoritative* fragment boundaries as far as this crate is
//! concerned. The spec says the comments and the `StartFragment`/`EndFragment`
//! byte counts are redundant; in practice they are not, because producers
//! recompute the byte counts after transforming the markup and get them wrong.
//! Microsoft's own documented MSHTML example is off by 141 bytes.

/// The literal `StartFragment` name, without the comment delimiters.
pub(crate) const START: &[u8] = b"StartFragment";
/// The literal `EndFragment` name, without the comment delimiters.
pub(crate) const END: &[u8] = b"EndFragment";

const OPEN: &[u8] = b"<!--";
const CLOSE: &[u8] = b"-->";

/// Find `<!--` *ws* `name` *ws* `-->` in `hay`.
///
/// Returns `(start, end)` of the whole comment, relative to `hay`.
///
/// Whitespace is tolerated inside the comment even though the spec forbids it,
/// because the spec forbids it in one section and then writes it both ways
/// itself: the grammar spells the marker `<!--StartFragment -->` and the
/// worked scenarios spell it `<!-- StartFragment-->`. Producers copied all
/// three spellings.
pub(crate) fn find(hay: &[u8], name: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0usize;
    while i + OPEN.len() <= hay.len() {
        if &hay[i..i + OPEN.len()] != OPEN {
            i += 1;
            continue;
        }
        let mut j = i + OPEN.len();
        j = skip_ws(hay, j);
        if hay.len() - j < name.len() || &hay[j..j + name.len()] != name {
            i += 1;
            continue;
        }
        j += name.len();
        j = skip_ws(hay, j);
        if hay.len() - j >= CLOSE.len() && &hay[j..j + CLOSE.len()] == CLOSE {
            return Some((i, j + CLOSE.len()));
        }
        i += 1;
    }
    None
}

fn skip_ws(hay: &[u8], mut i: usize) -> usize {
    while matches!(hay.get(i), Some(b) if b.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

/// Locate the fragment text between the two marker comments.
///
/// Returns `(fragment_start, fragment_end)` relative to `hay`: the byte just
/// past `<!--StartFragment-->` and the first byte of `<!--EndFragment-->`,
/// which is exactly what the `StartFragment` and `EndFragment` headers are
/// supposed to hold.
///
/// The end marker is searched for only *after* the start marker, so a fragment
/// that quotes the marker text before its own opening comment cannot move the
/// boundary backwards.
pub(crate) fn find_fragment(hay: &[u8]) -> Option<(usize, usize)> {
    let (_, start_end) = find(hay, START)?;
    let (end_start, _) = find(&hay[start_end..], END)?;
    Some((start_end, start_end + end_start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_spelling() {
        let hay = b"<html><!--StartFragment-->x<!--EndFragment--></html>";
        assert_eq!(find_fragment(hay), Some((26, 27)));
    }

    #[test]
    fn the_spellings_the_spec_uses_in_its_own_examples() {
        // Grammar section: trailing space. Scenario sections: leading space.
        let hay = b"<!--StartFragment -->x<!-- EndFragment-->";
        assert_eq!(find_fragment(hay), Some((21, 22)));
        let hay = b"<!--  StartFragment  -->y<!--  EndFragment  -->";
        assert_eq!(find_fragment(hay), Some((24, 25)));
    }

    #[test]
    fn a_lookalike_comment_does_not_match() {
        assert!(find(b"<!--StartFragmentation-->", START).is_none());
        assert!(
            find(b"<!--startfragment-->", START).is_none(),
            "the spec fixes the case"
        );
        assert!(
            find(b"<!--StartFragment--", START).is_none(),
            "unterminated"
        );
    }

    #[test]
    fn missing_end_marker_yields_nothing_rather_than_a_half_range() {
        assert!(find_fragment(b"<!--StartFragment-->x</html>").is_none());
    }

    #[test]
    fn end_marker_before_start_marker_is_ignored() {
        let hay = b"<!--EndFragment--><!--StartFragment-->x<!--EndFragment-->";
        assert_eq!(find_fragment(hay), Some((38, 39)));
    }
}

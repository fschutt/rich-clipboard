//! Which elements mean what.
//!
//! Four questions, four tables, all of them ASCII-case-insensitive because HTML
//! tag names are. The lists are deliberately short: this crate is scoped to the
//! elements that carry *character* formatting plus the ones that end a line,
//! and everything else is a transparent container whose text still comes
//! through.

/// A character-formatting element.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Formatting {
    /// `<b>`, `<strong>`.
    Bold,
    /// `<i>`, `<em>`, `<cite>`, `<var>`, `<dfn>`, `<address>`.
    Italic,
    /// `<u>`, `<ins>`.
    Underline,
    /// `<s>`, `<strike>`, `<del>`.
    Strike,
}

/// What formatting an element applies by itself, before any `style=`.
#[must_use]
pub fn formatting(name: &str) -> Option<Formatting> {
    let f = match () {
        () if eq(name, "b") || eq(name, "strong") => Formatting::Bold,
        () if eq(name, "i")
            || eq(name, "em")
            || eq(name, "cite")
            || eq(name, "var")
            || eq(name, "dfn")
            || eq(name, "address") =>
        {
            Formatting::Italic
        }
        () if eq(name, "u") || eq(name, "ins") => Formatting::Underline,
        () if eq(name, "s") || eq(name, "strike") || eq(name, "del") => Formatting::Strike,
        () => return None,
    };
    Some(f)
}

/// Elements a reader reopens when an enclosing element closes on top of them.
///
/// `<b><i>x</b>y</i>` renders `y` as italic in every browser: the `</b>` closes
/// the `<i>` too, and the `<i>` is then reconstructed under whatever `</b>`
/// left open. This is the *list* that HTML5's adoption agency algorithm works
/// on, applied by a much simpler rule — see [`crate::Runs`]. `<span>` and
/// `<div>` are deliberately not on it, exactly as in the standard: only the
/// character-formatting elements come back.
#[must_use]
pub fn is_formatting(name: &str) -> bool {
    const EXTRA: [&str; 7] = ["font", "big", "small", "tt", "nobr", "code", "a"];
    formatting(name).is_some() || EXTRA.iter().any(|k| eq(name, k))
}

/// `true` for `<h1>`..`<h6>` and `<th>`, which every renderer draws bold.
///
/// Applied because a heading that pastes at body weight is a visible loss and
/// the boldness is not an invention — it is in the UA stylesheet of every
/// browser that ever put the fragment on a clipboard. The *size* increase is
/// deliberately not applied: that one really would be inventing a number.
#[must_use]
pub fn is_bold_by_default(name: &str) -> bool {
    eq(name, "th")
        || (name.len() == 2
            && (name.as_bytes()[0] | 0x20) == b'h'
            && matches!(name.as_bytes()[1], b'1'..=b'6'))
}

/// Elements that never have content and never close.
///
/// An `<img>` left on the stack would swallow the rest of the document into it,
/// which for a fragment full of images is most of the fragment.
#[must_use]
pub fn is_void(name: &str) -> bool {
    const VOID: [&str; 16] = [
        "area", "base", "basefont", "br", "col", "embed", "frame", "hr", "img", "input", "keygen",
        "link", "meta", "param", "source", "track",
    ];
    VOID.iter().any(|k| eq(name, k)) || eq(name, "wbr")
}

/// Elements that end a line on both their open and close tags.
///
/// Table cells are not here — see [`is_cell`], which separates them with a tab
/// instead, because a table pasted one cell per line is less readable than the
/// original and a table pasted with the cells run together is unreadable.
#[must_use]
pub fn is_block(name: &str) -> bool {
    // `br` is deliberately absent: it is a line break rather than a block
    // boundary, and counting it as both makes one `<br>` two newlines.
    const BLOCK: [&str; 27] = [
        "address",
        "article",
        "aside",
        "blockquote",
        "caption",
        "center",
        "dd",
        "div",
        "dl",
        "dt",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "hr",
        "li",
        "main",
        "nav",
        "ol",
    ];
    const MORE: [&str; 8] = [
        "p", "pre", "section", "table", "tbody", "tfoot", "thead", "tr",
    ];
    BLOCK.iter().chain(MORE.iter()).any(|k| eq(name, k)) || eq(name, "ul")
}

/// `<td>` / `<th>`: separated from the next cell by a tab.
#[must_use]
pub fn is_cell(name: &str) -> bool {
    eq(name, "td") || eq(name, "th")
}

/// Elements whose text is not document text.
///
/// `<style>` and `<script>` are the ones that matter — every browser puts a
/// `<style>` block at the top of a clipboard fragment, and a reader that took
/// its contents as text pastes a stylesheet into the user's document.
#[must_use]
pub fn drops_content(name: &str) -> bool {
    const DROPPED: [&str; 10] = [
        "script", "style", "head", "title", "meta", "link", "template", "noscript", "iframe",
        "object",
    ];
    DROPPED.iter().any(|k| eq(name, k))
}

/// `<pre>` and `<textarea>`: whitespace is content.
#[must_use]
pub fn preserves_whitespace(name: &str) -> bool {
    eq(name, "pre") || eq(name, "textarea") || eq(name, "xmp") || eq(name, "plaintext")
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

//! Control-word classification tables.
//!
//! Two questions get asked about a control word, and both are answered here so
//! there is one place to look when the answer is wrong.

/// Is this control word the name of a destination whose content is *not*
/// document body text?
///
/// This drives two different decisions, and it has to be the same list for
/// both:
///
/// - `{\*\name ...}` — a `\*`-marked destination is dropped wholesale when we
///   do *not* recognise it. Recognising it here means we know what it is and
///   choose not to emit it, which for phase 0 has the same visible effect but
///   documents the difference.
/// - `{\name ...}` — an *unmarked* destination still must not have its content
///   emitted as body text. `{\fonttbl{\f0\fswiss Helvetica;}}` carries no `\*`,
///   and a reader that treats the unknown `\fonttbl` as "ignore the word, keep
///   the text" pastes `Helvetica;` into the user's document. That bug is
///   endemic; this list is the fix.
///
/// Conspicuously **absent**, because their content *is* body text:
/// `\fldrslt` (the rendered value of a field — the visible half of a
/// hyperlink), `\shptxt` and `\dptxbxtext` (text-box bodies), `\result` (an
/// embedded object's displayed result), and `\field` itself.
#[must_use]
pub fn is_known_destination(name: &str) -> bool {
    matches!(
        name,
        // Header tables.
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "listtable"
            | "listoverridetable"
            | "revtbl"
            | "rsidtbl"
            | "filetbl"
            | "file"
            | "generator"
            | "latentstyles"
            | "themedata"
            | "colorschememapping"
            | "datastore"
            | "xmlnstbl"
            | "defchp"
            | "defpap"
            // Font-table sub-destinations.
            | "falt"
            | "fname"
            | "panose"
            | "fontemb"
            | "fontfile"
            // Document properties.
            | "info"
            | "title"
            | "subject"
            | "author"
            | "manager"
            | "company"
            | "operator"
            | "category"
            | "keywords"
            | "comment"
            | "doccomm"
            | "hlinkbase"
            | "userprops"
            | "propname"
            | "staticval"
            | "password"
            | "passwordhash"
            | "template"
            | "keycode"
            // Pictures and embedded objects. `\pict` is *not* `\*`-marked in
            // older RTF, and its body is megabytes of hex digits.
            | "pict"
            | "nonshppict"
            | "object"
            | "objdata"
            | "objclass"
            | "objname"
            | "objalias"
            | "objsect"
            | "svblipuid"
            | "blipuid"
            // Out-of-flow text.
            | "header"
            | "headerl"
            | "headerr"
            | "headerf"
            | "footer"
            | "footerl"
            | "footerr"
            | "footerf"
            | "footnote"
            | "ftnsep"
            | "ftnsepc"
            | "ftncn"
            | "aftnsep"
            | "aftnsepc"
            | "aftncn"
            | "annotation"
            | "atnid"
            | "atnauthor"
            | "atndate"
            | "atnicn"
            | "atnref"
            | "atntime"
            | "atnparent"
            // Bookmarks and index/TOC entries: names, not prose.
            | "bkmkstart"
            | "bkmkend"
            | "xe"
            | "tc"
            | "tcn"
            // Field instructions, and the bullet/number text Word emits ahead
            // of a list paragraph for readers that do not understand `\pn`.
            | "fldinst"
            | "datafield"
            | "formfield"
            | "ffname"
            | "pn"
            | "pntext"
            | "pntxta"
            | "pntxtb"
            | "listtext"
            | "nesttableprops"
            | "nonesttables"
            // Drawing-object properties (the *text* destinations are absent
            // from this list on purpose).
            | "shpinst"
            | "shprslt"
            | "do"
    )
}

/// Control words that stand for exactly one character.
///
/// Word emits these constantly instead of `\uN` for the punctuation that has a
/// Windows-1252 representation, so dropping them turns `don't` into `dont`.
#[must_use]
pub fn symbol_char(name: &str) -> Option<char> {
    Some(match name {
        "emdash" => '\u{2014}',
        "endash" => '\u{2013}',
        "emspace" => '\u{2003}',
        "enspace" => '\u{2002}',
        "qmspace" => '\u{2005}',
        "bullet" => '\u{2022}',
        "lquote" => '\u{2018}',
        "rquote" => '\u{2019}',
        "ldblquote" => '\u{201C}',
        "rdblquote" => '\u{201D}',
        "zwbo" => '\u{200B}',
        "zwnbo" => '\u{2060}',
        "zwj" => '\u{200D}',
        "zwnj" => '\u{200C}',
        "ltrmark" => '\u{200E}',
        "rtlmark" => '\u{200F}',
        _ => return None,
    })
}

//! Character references: `&amp;`, `&#65;`, `&#x41;`.
//!
//! # Which named references
//!
//! The HTML5 named set has 2231 entries, more than half of which are
//! mathematical operators that no clipboard producer has ever emitted. This
//! table is the **HTML 4.01 set** — the Latin-1 block, the symbol and Greek
//! blocks, and the "special" block — plus the handful of HTML5 additions that
//! do turn up in real markup (`&apos;` above all, which HTML 4 lacked and every
//! XHTML-ish serializer emits). That is 284 entries, sorted, binary-searched,
//! and about 4 KB of rodata.
//!
//! A name outside the table is left alone: `&fakeentity;` stays the eight
//! characters it was written as, which is what a browser does with it and is
//! visibly wrong rather than silently missing.
//!
//! # The two rules that are not obvious
//!
//! - **A missing semicolon still resolves**, for names that have a defined
//!   longest match. `&nbsp` and `&amp` without the `;` are what hand-written
//!   HTML is full of, and every browser resolves them. `&notin` is `&notin;`
//!   and not `&not;` followed by `in`, so the match has to be longest-first.
//! - **Numeric references in `0x80..=0x9F` are Windows-1252, not C1 controls.**
//!   `&#150;` means an en dash, because the producer was a Windows application
//!   that confused a code page with Unicode. The HTML5 standard writes this
//!   mis-mapping into the spec precisely because every browser had to implement
//!   it; a parser that decodes `&#150;` to U+0096 produces an invisible control
//!   character where the document meant punctuation.

use rclip_core::Reader;

/// Longest named reference in [`NAMED`], plus room for the `&` and `;`.
///
/// Bounds the scan: `&` followed by 4 KB of letters is not a character
/// reference, and reading to the end of the buffer to find that out is how a
/// tokenizer becomes quadratic on hostile input.
const MAX_NAME: usize = 32;

/// Result of trying to read a character reference at a `&`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The decoded character.
    pub ch: char,
    /// Bytes consumed, including the `&` and any `;`.
    pub len: usize,
}

/// Decode the character reference starting at `at`, which must point at `&`.
///
/// Returns `None` when what follows is not a reference — in which case the `&`
/// is literal text, exactly as a browser treats it.
#[must_use]
pub fn decode(input: &[u8], at: usize) -> Option<Reference> {
    let r = Reader::new(input);
    let rest = r.tail_at(at).ok()?;
    if rest.first() != Some(&b'&') {
        return None;
    }
    let body = rest.get(1..)?;
    if body.first() == Some(&b'#') {
        return numeric(body);
    }
    named(body)
}

/// `&#123;` / `&#x1F600;`, with the Windows-1252 remap for the C1 range.
fn numeric(body: &[u8]) -> Option<Reference> {
    // `body` starts at `#`.
    // `head` is what precedes the digits inside `body`: `#` or `#x`.
    let (digits, radix, head) = match body.get(1) {
        Some(b'x' | b'X') => (body.get(2..)?, 16u32, 2usize),
        _ => (body.get(1..)?, 10u32, 1usize),
    };
    let mut value: u32 = 0;
    let mut used = 0usize;
    for &b in digits {
        let d = match (radix, b) {
            (_, b'0'..=b'9') => u32::from(b - b'0'),
            (16, b'a'..=b'f') => u32::from(b - b'a') + 10,
            (16, b'A'..=b'F') => u32::from(b - b'A') + 10,
            _ => break,
        };
        // Saturating rather than wrapping: a reference with forty digits is
        // junk, and junk that wrapped to a valid scalar would be worse than
        // junk that clamps to the replacement character.
        value = value.saturating_mul(radix).saturating_add(d);
        used += 1;
        if used > 8 {
            // Enough digits to have overflowed anything meaningful. Keep
            // consuming — every digit belongs to the reference and leaving the
            // tail behind would paste it as text — but stop accumulating, and
            // let `scalar` turn it into U+FFFD.
            value = u32::MAX;
        }
    }
    if used == 0 {
        return None;
    }
    // The `;` is optional here too — `&#65` is a parse error the spec says to
    // recover from by decoding it anyway.
    let semi = usize::from(digits.get(used) == Some(&b';'));
    Some(Reference {
        ch: scalar(value),
        // The `&`, the `#`/`#x`, the digits, and the `;` if there was one.
        len: 1 + head + used + semi,
    })
}

/// Map a numeric reference's value to a character, HTML5's rules included.
fn scalar(value: u32) -> char {
    if let Some(c) = windows1252_c1(value) {
        return c;
    }
    match value {
        // NUL, and anything past the last scalar value.
        0 | 0x11_0000.. => char::REPLACEMENT_CHARACTER,
        // Surrogates are not characters; `char::from_u32` already refuses them,
        // and the replacement character is what a browser substitutes.
        v => char::from_u32(v).unwrap_or(char::REPLACEMENT_CHARACTER),
    }
}

/// `0x80..=0x9F` are C1 control codes in Unicode and printable characters in
/// Windows-1252, and HTML5 mandates the second reading.
fn windows1252_c1(value: u32) -> Option<char> {
    let c = match value {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        _ => return None,
    };
    Some(c)
}

/// A named reference, longest match first.
fn named(body: &[u8]) -> Option<Reference> {
    let len = body
        .iter()
        .take(MAX_NAME)
        .take_while(|b| b.is_ascii_alphanumeric())
        .count();
    if len == 0 {
        return None;
    }
    let name = body.get(..len)?;
    // With the `;`, only the exact name counts.
    if body.get(len) == Some(&b';') {
        if let Some(ch) = lookup(name) {
            return Some(Reference {
                ch,
                len: 1 + len + 1,
            });
        }
        return None;
    }
    // Without it, the longest defined prefix wins. `&notin` is `&notin;`, not
    // `&not;` followed by `in`.
    for take in (1..=len).rev() {
        if let Some(ch) = lookup(body.get(..take)?) {
            return Some(Reference { ch, len: 1 + take });
        }
    }
    None
}

fn lookup(name: &[u8]) -> Option<char> {
    let name = core::str::from_utf8(name).ok()?;
    NAMED
        .binary_search_by(|(k, _)| (*k).cmp(name))
        .ok()
        .map(|i| NAMED[i].1)
}

/// The named references this crate resolves, sorted by name.
///
/// Sortedness is load-bearing — [`lookup`] binary-searches it — and is asserted
/// by a test rather than trusted.
pub static NAMED: &[(&str, char)] = &[
    ("AElig", '\u{00c6}'),
    ("Aacute", '\u{00c1}'),
    ("Acirc", '\u{00c2}'),
    ("Agrave", '\u{00c0}'),
    ("Alpha", '\u{0391}'),
    ("Aring", '\u{00c5}'),
    ("Atilde", '\u{00c3}'),
    ("Auml", '\u{00c4}'),
    ("Beta", '\u{0392}'),
    ("Ccedil", '\u{00c7}'),
    ("Chi", '\u{03a7}'),
    ("Dagger", '\u{2021}'),
    ("Delta", '\u{0394}'),
    ("ETH", '\u{00d0}'),
    ("Eacute", '\u{00c9}'),
    ("Ecirc", '\u{00ca}'),
    ("Egrave", '\u{00c8}'),
    ("Epsilon", '\u{0395}'),
    ("Eta", '\u{0397}'),
    ("Euml", '\u{00cb}'),
    ("Gamma", '\u{0393}'),
    ("Iacute", '\u{00cd}'),
    ("Icirc", '\u{00ce}'),
    ("Igrave", '\u{00cc}'),
    ("Iota", '\u{0399}'),
    ("Iuml", '\u{00cf}'),
    ("Kappa", '\u{039a}'),
    ("Lambda", '\u{039b}'),
    ("Mu", '\u{039c}'),
    ("NewLine", '\u{000a}'),
    ("Ntilde", '\u{00d1}'),
    ("Nu", '\u{039d}'),
    ("OElig", '\u{0152}'),
    ("Oacute", '\u{00d3}'),
    ("Ocirc", '\u{00d4}'),
    ("Ograve", '\u{00d2}'),
    ("Omega", '\u{03a9}'),
    ("Omicron", '\u{039f}'),
    ("Oslash", '\u{00d8}'),
    ("Otilde", '\u{00d5}'),
    ("Ouml", '\u{00d6}'),
    ("Phi", '\u{03a6}'),
    ("Pi", '\u{03a0}'),
    ("Prime", '\u{2033}'),
    ("Psi", '\u{03a8}'),
    ("Rho", '\u{03a1}'),
    ("Scaron", '\u{0160}'),
    ("Sigma", '\u{03a3}'),
    ("THORN", '\u{00de}'),
    ("Tab", '\u{0009}'),
    ("Tau", '\u{03a4}'),
    ("Theta", '\u{0398}'),
    ("Uacute", '\u{00da}'),
    ("Ucirc", '\u{00db}'),
    ("Ugrave", '\u{00d9}'),
    ("Upsilon", '\u{03a5}'),
    ("Uuml", '\u{00dc}'),
    ("Xi", '\u{039e}'),
    ("Yacute", '\u{00dd}'),
    ("Yuml", '\u{0178}'),
    ("Zeta", '\u{0396}'),
    ("aacute", '\u{00e1}'),
    ("acirc", '\u{00e2}'),
    ("acute", '\u{00b4}'),
    ("aelig", '\u{00e6}'),
    ("agrave", '\u{00e0}'),
    ("alefsym", '\u{2135}'),
    ("alpha", '\u{03b1}'),
    ("amp", '\u{0026}'),
    ("and", '\u{2227}'),
    ("ang", '\u{2220}'),
    ("apos", '\u{0027}'),
    ("aring", '\u{00e5}'),
    ("ast", '\u{002a}'),
    ("asymp", '\u{2248}'),
    ("atilde", '\u{00e3}'),
    ("auml", '\u{00e4}'),
    ("bdquo", '\u{201e}'),
    ("beta", '\u{03b2}'),
    ("blank", '\u{2423}'),
    ("brvbar", '\u{00a6}'),
    ("bull", '\u{2022}'),
    ("cap", '\u{2229}'),
    ("ccedil", '\u{00e7}'),
    ("cedil", '\u{00b8}'),
    ("cent", '\u{00a2}'),
    ("check", '\u{2713}'),
    ("chi", '\u{03c7}'),
    ("circ", '\u{02c6}'),
    ("clubs", '\u{2663}'),
    ("colon", '\u{003a}'),
    ("comma", '\u{002c}'),
    ("commat", '\u{0040}'),
    ("cong", '\u{2245}'),
    ("copy", '\u{00a9}'),
    ("crarr", '\u{21b5}'),
    ("cross", '\u{2717}'),
    ("cup", '\u{222a}'),
    ("curren", '\u{00a4}'),
    ("dArr", '\u{21d3}'),
    ("dagger", '\u{2020}'),
    ("darr", '\u{2193}'),
    ("deg", '\u{00b0}'),
    ("delta", '\u{03b4}'),
    ("diams", '\u{2666}'),
    ("divide", '\u{00f7}'),
    ("dollar", '\u{0024}'),
    ("eacute", '\u{00e9}'),
    ("ecirc", '\u{00ea}'),
    ("egrave", '\u{00e8}'),
    ("empty", '\u{2205}'),
    ("emsp", '\u{2003}'),
    ("ensp", '\u{2002}'),
    ("epsilon", '\u{03b5}'),
    ("equals", '\u{003d}'),
    ("equiv", '\u{2261}'),
    ("eta", '\u{03b7}'),
    ("eth", '\u{00f0}'),
    ("euml", '\u{00eb}'),
    ("euro", '\u{20ac}'),
    ("excl", '\u{0021}'),
    ("exist", '\u{2203}'),
    ("fnof", '\u{0192}'),
    ("forall", '\u{2200}'),
    ("frac12", '\u{00bd}'),
    ("frac14", '\u{00bc}'),
    ("frac34", '\u{00be}'),
    ("frasl", '\u{2044}'),
    ("gamma", '\u{03b3}'),
    ("ge", '\u{2265}'),
    ("grave", '\u{0060}'),
    ("gt", '\u{003e}'),
    ("hArr", '\u{21d4}'),
    ("half", '\u{00bd}'),
    ("harr", '\u{2194}'),
    ("hearts", '\u{2665}'),
    ("hellip", '\u{2026}'),
    ("iacute", '\u{00ed}'),
    ("icirc", '\u{00ee}'),
    ("iexcl", '\u{00a1}'),
    ("igrave", '\u{00ec}'),
    ("image", '\u{2111}'),
    ("infin", '\u{221e}'),
    ("int", '\u{222b}'),
    ("iota", '\u{03b9}'),
    ("iquest", '\u{00bf}'),
    ("isin", '\u{2208}'),
    ("iuml", '\u{00ef}'),
    ("kappa", '\u{03ba}'),
    ("lArr", '\u{21d0}'),
    ("lambda", '\u{03bb}'),
    ("lang", '\u{2329}'),
    ("laquo", '\u{00ab}'),
    ("larr", '\u{2190}'),
    ("lceil", '\u{2308}'),
    ("lcub", '\u{007b}'),
    ("ldquo", '\u{201c}'),
    ("le", '\u{2264}'),
    ("lfloor", '\u{230a}'),
    ("lowast", '\u{2217}'),
    ("lowbar", '\u{005f}'),
    ("loz", '\u{25ca}'),
    ("lpar", '\u{0028}'),
    ("lrm", '\u{200e}'),
    ("lsaquo", '\u{2039}'),
    ("lsqb", '\u{005b}'),
    ("lsquo", '\u{2018}'),
    ("lt", '\u{003c}'),
    ("macr", '\u{00af}'),
    ("mdash", '\u{2014}'),
    ("micro", '\u{00b5}'),
    ("midast", '\u{002a}'),
    ("middot", '\u{00b7}'),
    ("minus", '\u{2212}'),
    ("mu", '\u{03bc}'),
    ("nabla", '\u{2207}'),
    ("nbsp", '\u{00a0}'),
    ("ndash", '\u{2013}'),
    ("ne", '\u{2260}'),
    ("ni", '\u{220b}'),
    ("not", '\u{00ac}'),
    ("notin", '\u{2209}'),
    ("nsub", '\u{2284}'),
    ("ntilde", '\u{00f1}'),
    ("nu", '\u{03bd}'),
    ("num", '\u{0023}'),
    ("oacute", '\u{00f3}'),
    ("ocirc", '\u{00f4}'),
    ("oelig", '\u{0153}'),
    ("ograve", '\u{00f2}'),
    ("oline", '\u{203e}'),
    ("omega", '\u{03c9}'),
    ("omicron", '\u{03bf}'),
    ("oplus", '\u{2295}'),
    ("or", '\u{2228}'),
    ("ordf", '\u{00aa}'),
    ("ordm", '\u{00ba}'),
    ("oslash", '\u{00f8}'),
    ("otilde", '\u{00f5}'),
    ("otimes", '\u{2297}'),
    ("ouml", '\u{00f6}'),
    ("para", '\u{00b6}'),
    ("part", '\u{2202}'),
    ("percnt", '\u{0025}'),
    ("period", '\u{002e}'),
    ("permil", '\u{2030}'),
    ("perp", '\u{22a5}'),
    ("phi", '\u{03c6}'),
    ("pi", '\u{03c0}'),
    ("piv", '\u{03d6}'),
    ("plus", '\u{002b}'),
    ("plusmn", '\u{00b1}'),
    ("pound", '\u{00a3}'),
    ("prime", '\u{2032}'),
    ("prod", '\u{220f}'),
    ("prop", '\u{221d}'),
    ("psi", '\u{03c8}'),
    ("quest", '\u{003f}'),
    ("quot", '\u{0022}'),
    ("rArr", '\u{21d2}'),
    ("radic", '\u{221a}'),
    ("rang", '\u{232a}'),
    ("raquo", '\u{00bb}'),
    ("rarr", '\u{2192}'),
    ("rceil", '\u{2309}'),
    ("rcub", '\u{007d}'),
    ("rdquo", '\u{201d}'),
    ("real", '\u{211c}'),
    ("reg", '\u{00ae}'),
    ("rfloor", '\u{230b}'),
    ("rho", '\u{03c1}'),
    ("rlm", '\u{200f}'),
    ("rpar", '\u{0029}'),
    ("rsaquo", '\u{203a}'),
    ("rsqb", '\u{005d}'),
    ("rsquo", '\u{2019}'),
    ("sbquo", '\u{201a}'),
    ("scaron", '\u{0161}'),
    ("sdot", '\u{22c5}'),
    ("sect", '\u{00a7}'),
    ("semi", '\u{003b}'),
    ("shy", '\u{00ad}'),
    ("sigma", '\u{03c3}'),
    ("sigmaf", '\u{03c2}'),
    ("sim", '\u{223c}'),
    ("sol", '\u{002f}'),
    ("spades", '\u{2660}'),
    ("starf", '\u{2605}'),
    ("sub", '\u{2282}'),
    ("sube", '\u{2286}'),
    ("sum", '\u{2211}'),
    ("sup", '\u{2283}'),
    ("sup1", '\u{00b9}'),
    ("sup2", '\u{00b2}'),
    ("sup3", '\u{00b3}'),
    ("supe", '\u{2287}'),
    ("szlig", '\u{00df}'),
    ("tau", '\u{03c4}'),
    ("there4", '\u{2234}'),
    ("theta", '\u{03b8}'),
    ("thetasym", '\u{03d1}'),
    ("thinsp", '\u{2009}'),
    ("thorn", '\u{00fe}'),
    ("tilde", '\u{02dc}'),
    ("times", '\u{00d7}'),
    ("trade", '\u{2122}'),
    ("uArr", '\u{21d1}'),
    ("uacute", '\u{00fa}'),
    ("uarr", '\u{2191}'),
    ("ucirc", '\u{00fb}'),
    ("ugrave", '\u{00f9}'),
    ("uml", '\u{00a8}'),
    ("upsih", '\u{03d2}'),
    ("upsilon", '\u{03c5}'),
    ("uuml", '\u{00fc}'),
    ("verbar", '\u{007c}'),
    ("weierp", '\u{2118}'),
    ("xi", '\u{03be}'),
    ("yacute", '\u{00fd}'),
    ("yen", '\u{00a5}'),
    ("yuml", '\u{00ff}'),
    ("zeta", '\u{03b6}'),
    ("zwj", '\u{200d}'),
    ("zwnj", '\u{200c}'),
];

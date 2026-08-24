//! Integration tests for `rclip-codepage`.
//!
//! Three things get the most attention, because they are the three ways a code
//! page table goes wrong without anyone noticing: the Windows-1252 versus
//! ISO-8859-1 split at `0x80..=0x9F`, the bytes a code page leaves undefined,
//! and the reverse map being single-valued.

use std::{collections::BTreeSet, fs};

use rclip_codepage::{Encoding, ErrorKind};

const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/synthetic/rclip-codepage/"
);

// ---------------------------------------------------------------- sidecars

/// The one string value of `"key": "..."` in a sidecar, unescaped.
///
/// A hand-rolled reader rather than a dependency: the sidecars are flat objects
/// written by one script, and this crate's whole point is not taking
/// dependencies for table-shaped problems.
fn json_str(src: &str, key: &str) -> Option<String> {
    let at = src.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = &src[at..];
    let open = rest.find('"')? + 1;
    let mut out = String::new();
    let mut it = rest[open..].chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(out),
            '\\' => match it.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' => {
                    let hex: String = it.by_ref().take(4).collect();
                    let v = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(v)?);
                }
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

/// The one numeric value of `"key": 123` in a sidecar.
fn json_num(src: &str, key: &str) -> Option<u32> {
    let at = src.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = src[at..].trim_start_matches([':', ' ']);
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

struct Fixture {
    name: String,
    bytes: Vec<u8>,
    sidecar: String,
}

fn fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();
    for entry in fs::read_dir(CORPUS).expect("corpus directory") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("utf-8 name")
            .to_owned();
        let sidecar = fs::read_to_string(path.with_extension("json"))
            .unwrap_or_else(|e| panic!("every .bin needs a .json sidecar: {name}: {e}"));
        out.push(Fixture {
            name,
            bytes: fs::read(&path).expect("fixture bytes"),
            sidecar,
        });
    }
    assert!(!out.is_empty(), "corpus directory should not be empty");
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

// ---------------------------------------------------------------- corpus

#[test]
fn every_fixture_decodes_as_its_sidecar_says() {
    let mut seen_ok = 0;
    let mut seen_err = 0;
    for f in fixtures() {
        let cp = json_num(&f.sidecar, "expect_codepage")
            .unwrap_or_else(|| panic!("{}: sidecar needs expect_codepage", f.name));
        let enc = Encoding::from_windows_codepage(cp)
            .unwrap_or_else(|| panic!("{}: code page {cp} is not implemented", f.name));

        match json_str(&f.sidecar, "expect").as_deref() {
            Some("ok") => {
                seen_ok += 1;
                let want = json_str(&f.sidecar, "expect_decoded")
                    .unwrap_or_else(|| panic!("{}: an ok fixture needs expect_decoded", f.name));
                let got = enc
                    .decode_to_string(&f.bytes)
                    .unwrap_or_else(|e| panic!("{}: should decode cleanly, got {e}", f.name));
                assert_eq!(
                    got, want,
                    "{}: decoded text should match the sidecar oracle",
                    f.name
                );
            }
            Some("error") => {
                seen_err += 1;
                let at = json_num(&f.sidecar, "expect_error_offset")
                    .expect("an error fixture needs expect_error_offset")
                    as usize;
                let err = enc
                    .decode_to_string(&f.bytes)
                    .expect_err("an undefined byte must not decode");
                assert_eq!(
                    err.kind,
                    ErrorKind::Malformed,
                    "{}: an undefined byte is Malformed, not Unsupported: the code page is \
                     implemented, the byte just has no character",
                    f.name
                );
                assert_eq!(err.offset, at, "{}: error offset", f.name);
            }
            other => panic!("{}: expect must be ok or error, got {other:?}", f.name),
        }
    }
    assert!(
        seen_ok >= 12,
        "expected a fixture per encoding, got {seen_ok}"
    );
    assert!(seen_err >= 2, "expected malformed fixtures, got {seen_err}");
}

#[test]
fn the_same_bytes_decode_differently_under_1252_and_latin1() {
    let bytes = fs::read(format!("{CORPUS}cp1252-smart-quotes.bin")).expect("fixture");
    let other = fs::read(format!("{CORPUS}latin1-same-bytes-as-cp1252.bin")).expect("fixture");
    assert_eq!(
        bytes, other,
        "the two fixtures are deliberately byte-identical; only the code page differs"
    );

    let win = Encoding::Windows1252
        .decode_to_string(&bytes)
        .expect("1252");
    let lat = Encoding::Iso8859_1
        .decode_to_string(&bytes)
        .expect("latin1");
    assert_ne!(
        win, lat,
        "confusing Windows-1252 with ISO-8859-1 is the most common mojibake source in \
         clipboard text; these must not agree"
    );
    assert!(win.contains('\u{201C}'), "1252 0x93 is a left double quote");
    assert!(lat.contains('\u{0093}'), "Latin-1 0x93 is a C1 control");
}

// ---------------------------------------------------------- table invariants

#[test]
fn every_encoding_is_ascii_transparent() {
    for &enc in Encoding::ALL {
        for b in 0..0x80u8 {
            assert_eq!(
                enc.decode_byte(b),
                Some(char::from(b)),
                "{}: byte {b:#04X} must decode to itself; the 128-entry table layout \
                 depends on it",
                enc.name()
            );
        }
    }
}

#[test]
fn undefined_bytes_are_reported_not_substituted() {
    // Counted off the Unicode Consortium's mapping files. A change here means
    // the tables were regenerated from a different upstream revision, which
    // should be a deliberate commit and not a surprise.
    let expected: &[(Encoding, &[u8])] = &[
        (Encoding::Iso8859_1, &[]),
        (Encoding::Windows1250, &[0x81, 0x83, 0x88, 0x90, 0x98]),
        (Encoding::Windows1251, &[0x98]),
        (Encoding::Windows1252, &[0x81, 0x8D, 0x8F, 0x90, 0x9D]),
        (Encoding::Windows1256, &[]),
        (Encoding::Cp437, &[]),
        (Encoding::Cp850, &[]),
        (Encoding::MacRoman, &[]),
    ];
    for &(enc, want) in expected {
        let got: Vec<u8> = (0..=0xFFu8).filter(|&b| !enc.is_defined(b)).collect();
        assert_eq!(got, want.to_vec(), "{}: undefined byte set", enc.name());
        assert_eq!(
            enc.has_undefined_bytes(),
            !want.is_empty(),
            "{}: has_undefined_bytes must agree with the table",
            enc.name()
        );
        for &b in want {
            assert_eq!(
                enc.decode_byte_lossy(b),
                '\u{FFFD}',
                "{}: the lossy path substitutes, the strict one does not",
                enc.name()
            );
        }
    }
}

#[test]
fn windows_1255_has_the_most_undefined_bytes() {
    // 23 of them, which is why "just substitute U+FFFD" is not an acceptable
    // default: a Hebrew payload decoded with a wrong-but-plausible page would
    // come back looking merely damaged rather than wrong.
    let n = (0..=0xFFu8)
        .filter(|&b| !Encoding::Windows1255.is_defined(b))
        .count();
    assert_eq!(n, 23, "windows-1255 undefined byte count");
}

#[test]
fn no_table_entry_is_a_surrogate_or_out_of_range() {
    for &enc in Encoding::ALL {
        let Some(table) = enc.high_table() else {
            continue;
        };
        for (i, &cp) in table.iter().enumerate() {
            if cp == 0 {
                continue; // the undefined sentinel
            }
            assert!(
                char::from_u32(u32::from(cp)).is_some(),
                "{}: 0x{:02X} maps to U+{cp:04X}, which is not a scalar value",
                enc.name(),
                i + 0x80
            );
        }
    }
}

#[test]
fn the_reverse_map_is_single_valued() {
    // If two bytes shared a target, `encode_char` would have to pick one and
    // the round-trip below would fail for the other. The generator rejects such
    // a table; this proves the shipped constants match that promise.
    for &enc in Encoding::ALL {
        let mut seen = BTreeSet::new();
        for b in 0x80..=0xFFu8 {
            if let Some(c) = enc.decode_byte(b) {
                assert!(
                    seen.insert(c),
                    "{}: U+{:04X} is the target of more than one byte",
                    enc.name(),
                    c as u32
                );
            }
        }
    }
}

#[test]
fn decode_then_encode_returns_the_same_byte() {
    for &enc in Encoding::ALL {
        for b in 0..=0xFFu8 {
            let Some(c) = enc.decode_byte(b) else {
                continue;
            };
            assert_eq!(
                enc.encode_char(c),
                Some(b),
                "{}: byte {b:#04X} -> U+{:04X} -> back",
                enc.name(),
                c as u32
            );
        }
    }
}

#[test]
fn encode_rejects_a_character_the_page_lacks() {
    // U+4E00 is in no single-byte page anywhere.
    for &enc in Encoding::ALL {
        assert_eq!(
            enc.encode_char('\u{4E00}'),
            None,
            "{}: a CJK ideograph has no single-byte form",
            enc.name()
        );
    }
    // The euro exists in 1252 and Mac Roman but not in Latin-1 or CP437.
    assert_eq!(Encoding::Windows1252.encode_char('\u{20AC}'), Some(0x80));
    assert_eq!(Encoding::MacRoman.encode_char('\u{20AC}'), Some(0xDB));
    assert_eq!(Encoding::Iso8859_1.encode_char('\u{20AC}'), None);
    assert_eq!(Encoding::Cp437.encode_char('\u{20AC}'), None);
}

// ---------------------------------------------------------- code page numbers

#[test]
fn codepage_numbers_round_trip() {
    for &enc in Encoding::ALL {
        let n = u32::from(enc.windows_codepage());
        assert_eq!(
            Encoding::from_windows_codepage(n),
            Some(enc),
            "{}: code page {n} must map back",
            enc.name()
        );
    }
    // 819 is IBM's number for Latin-1 and appears in older RTF `\ansicpg`.
    assert_eq!(
        Encoding::from_windows_codepage(819),
        Some(Encoding::Iso8859_1)
    );
}

#[test]
fn multibyte_and_unicode_codepages_are_not_pretended_to_be_single_byte() {
    // Applying a single-byte table to any of these produces confident garbage,
    // which is worse than an error, so the lookup refuses.
    for n in [0, 1, 932, 936, 949, 950, 1200, 1201, 65000, 65001, 99999] {
        assert_eq!(
            Encoding::from_windows_codepage(n),
            None,
            "code page {n} is not a single-byte page this crate implements"
        );
    }
}

// ------------------------------------------------------------- the C1 dispute

#[test]
fn lenient_mode_follows_windows_on_the_c1_holes_only() {
    // The Unicode mapping files leave these undefined; MultiByteToWideChar and
    // the WHATWG index map them to the C1 control of the same value. Both
    // behaviours are reachable, neither is silent.
    for b in [0x81u8, 0x8D, 0x8F, 0x90, 0x9D] {
        assert_eq!(Encoding::Windows1252.decode_byte(b), None);
        assert_eq!(
            Encoding::Windows1252.decode_byte_lenient(b),
            char::from_u32(u32::from(b))
        );
    }
    // Outside the C1 range there is no "same value" fallback, so lenient mode
    // changes nothing: Greek 0xAA stays undefined either way.
    assert_eq!(Encoding::Windows1253.decode_byte(0xAA), None);
    assert_eq!(Encoding::Windows1253.decode_byte_lenient(0xAA), None);
    // Windows-1255 0xCA is the one substantive disagreement with WHATWG, which
    // maps it to U+05BA. The pinned Microsoft file does not, and lenient mode
    // deliberately does not paper over it.
    assert_eq!(Encoding::Windows1255.decode_byte(0xCA), None);
    assert_eq!(Encoding::Windows1255.decode_byte_lenient(0xCA), None);
}

// ------------------------------------------------------------------ iterators

#[test]
fn an_undefined_byte_does_not_stop_the_iterator() {
    // A single-byte encoding cannot lose sync, so unlike UTF-16 there is no
    // reason to abandon the rest of the field after one bad byte.
    let items: Vec<_> = Encoding::Windows1252.decode(b"a\x81b\x8Dc").collect();
    assert_eq!(items.len(), 5, "one item per byte, error or not");
    assert_eq!(items[0], Ok('a'));
    assert_eq!(items[1].unwrap_err().offset, 1);
    assert_eq!(items[2], Ok('b'));
    assert_eq!(items[3].unwrap_err().offset, 3);
    assert_eq!(items[4], Ok('c'));
}

#[test]
fn decoder_length_is_exact() {
    let bytes = b"\x80\x81\x82";
    let mut it = Encoding::Windows1252.decode(bytes);
    assert_eq!(it.len(), 3);
    let _ = it.next();
    assert_eq!(it.len(), 2);
    assert_eq!(it.remaining(), b"\x81\x82");

    let mut empty = Encoding::Windows1252.decode(b"");
    assert_eq!(empty.len(), 0);
    assert_eq!(empty.next(), None, "an empty slice yields nothing at all");
}

#[test]
fn lossy_decoding_substitutes_exactly_the_undefined_bytes() {
    let s = Encoding::Windows1252.decode_to_string_lossy(b"a\x81b");
    assert_eq!(s, "a\u{FFFD}b");
    assert_eq!(
        s.chars().count(),
        3,
        "the lossy iterator stays one char per byte"
    );
}

#[test]
fn encode_from_str_reports_the_byte_offset_of_the_bad_character() {
    let err = Encoding::Iso8859_1
        .encode_from_str("caf\u{E9} \u{20AC}")
        .expect_err("the euro has no Latin-1 byte");
    assert_eq!(err.kind, ErrorKind::Unsupported);
    // "caf" + 2 bytes for U+00E9 + 1 for the space.
    assert_eq!(err.offset, 6, "offset is into the &str, in bytes");

    assert_eq!(
        Encoding::Windows1252
            .encode_from_str("caf\u{E9} \u{20AC}")
            .expect("1252 has both"),
        b"caf\xE9 \x80"
    );
}

// -------------------------------------------------------------- spot checks

#[test]
fn mac_roman_carries_the_euro_and_the_apple_logo() {
    // 0xDB was CURRENCY SIGN before Mac OS 8.5. ROMAN.TXT revision c02, which
    // the tables are pinned to, says EURO SIGN; so does the WHATWG index.
    assert_eq!(Encoding::MacRoman.decode_byte(0xDB), Some('\u{20AC}'));
    // Apple's logo has no Unicode character; it lives in the private use area.
    assert_eq!(Encoding::MacRoman.decode_byte(0xF0), Some('\u{F8FF}'));
    // 0xBD is the canonical decomposition of OHM SIGN, not U+2126 itself.
    assert_eq!(Encoding::MacRoman.decode_byte(0xBD), Some('\u{03A9}'));
}

#[test]
fn combining_marks_stay_one_byte_to_one_char() {
    // Vietnamese tone marks and Hebrew points are separate characters in these
    // pages. The mapping is still 1:1 -- what changes is that char count no
    // longer tracks grapheme count, which is the trap for anything truncating.
    assert_eq!(Encoding::Windows1258.decode_byte(0xEC), Some('\u{0301}'));
    assert_eq!(Encoding::Windows1258.decode_byte(0xF2), Some('\u{0323}'));
    assert_eq!(Encoding::Windows1255.decode_byte(0xC8), Some('\u{05B8}'));

    let s = Encoding::Windows1258
        .decode_to_string(b"Ti\xEA\xECng")
        .expect("all defined");
    assert_eq!(s.chars().count(), 6, "one char per byte");
    assert!(
        s.chars().nth(3) == Some('\u{0301}'),
        "the tone mark is its own char and follows its vowel"
    );
}

#[test]
fn cp437_and_cp850_disagree_where_they_should() {
    // Both are complete, and both are ASCII-transparent, so the only way to
    // tell them apart is the upper half -- which is why RTF distinguishes \pc
    // from \pca instead of treating "OEM" as one thing.
    let differing = (0x80..=0xFFu8)
        .filter(|&b| Encoding::Cp437.decode_byte(b) != Encoding::Cp850.decode_byte(b))
        .count();
    assert_eq!(
        differing, 47,
        "437 and 850 share 0x80-0x9A and diverge from 0x9B up; a change here means the \
         tables were regenerated from a different upstream revision"
    );
    assert_eq!(Encoding::Cp437.decode_byte(0xDB), Some('\u{2588}')); // full block
    assert_eq!(Encoding::Cp850.decode_byte(0xDB), Some('\u{2588}')); // shared
    assert_eq!(Encoding::Cp437.decode_byte(0xB5), Some('\u{2561}')); // box drawing
    assert_eq!(Encoding::Cp850.decode_byte(0xB5), Some('\u{00C1}')); // A acute
}

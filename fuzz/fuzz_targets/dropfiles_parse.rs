//! `CF_HDROP` / `DROPFILES` — `rclip_dropfiles::DropFiles::parse`.
//!
//! `pFiles` is an attacker-supplied offset into the payload, and the path array
//! ends at a double NUL whose position is found by walking. Both are the
//! classic CF_HDROP bugs, in both directions.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_dropfiles::DropFiles;

fuzz_target!(|data: &[u8]| {
    let Ok(df) = DropFiles::parse(data) else {
        return;
    };

    let paths: Vec<&[u8]> = df.paths().map(|p| p.as_bytes()).collect();
    assert_eq!(paths.len(), df.count());
    assert_eq!(df.is_empty(), paths.is_empty());
    for p in df.paths() {
        assert_eq!(p.is_wide(), df.is_wide());
        // Wide paths decode a `char` at a time; a lone surrogate must come back
        // as an `Err` item rather than blowing up the iterator.
        if let Some(chars) = p.chars() {
            for c in chars {
                let _ = c;
            }
        }
        let _ = p.to_string_lossy();
    }

    // Round trip. Canonical by construction: `to_bytes` always writes
    // `pFiles == 20` with no gap, so a payload that carried a gap between the
    // header and the array does not come back byte-identical. What must hold is
    // that re-parsing gives an equal value, and that the second serialization
    // is a fixed point.
    let bytes = df.to_bytes();
    let back = DropFiles::parse(&bytes).expect("our own output must parse");
    assert_eq!(back.point(), df.point());
    assert_eq!(back.is_non_client(), df.is_non_client());
    assert_eq!(back.is_wide(), df.is_wide());
    assert_eq!(back.raw_list(), df.raw_list());
    let back_paths: Vec<&[u8]> = back.paths().map(|p| p.as_bytes()).collect();
    assert_eq!(back_paths, paths);
    assert_eq!(back.to_bytes(), bytes);

    // And the stronger property where it does apply: a fully canonical payload
    // must survive byte for byte.
    //
    // "Canonical" here is narrower than `pFiles == 20`. fNC and fWide are Win32
    // `BOOL`, i.e. `int`, and the parser correctly treats any nonzero value as
    // TRUE -- real sources write -1 and 0xFFFFFFFF -- but `to_bytes` writes them
    // back as 1. So a header carrying 0x80000001 in fWide round-trips to an
    // equal *value* and different *bytes*, which is the writer normalising and
    // not a bug. Found by this target on its first run.
    let bool_word = |at: usize| -> Option<u32> {
        data.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let canonical_bools = matches!(bool_word(12), Some(0 | 1)) && matches!(bool_word(16), Some(0 | 1));
    if df.list_offset() == 20 && canonical_bools && data.len() == bytes.len() {
        assert_eq!(bytes, data, "canonical payload did not round-trip exactly");
    }
});

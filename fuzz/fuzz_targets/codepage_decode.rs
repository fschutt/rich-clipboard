//! Legacy single-byte code pages — `rclip_codepage::Encoding`'s decoders.
//!
//! These bytes are as attacker-supplied as any other parser's: an ANSI path out
//! of `CF_HDROP` and an RTF `\'hh` run both land here, with the code page named
//! by the *source* machine and not verifiable from the payload.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_codepage::Encoding;

fuzz_target!(|data: &[u8]| {
    for &enc in Encoding::ALL {
        // Documented invariant: exactly one `char` per input byte, whether or
        // not that byte is defined. A caller that sized a buffer from
        // `bytes.len()` is relying on it.
        let lossy: Vec<char> = enc.decode_lossy(data).collect();
        assert_eq!(lossy.len(), data.len(), "{}: not one char per byte", enc.name());

        let strict: Vec<_> = enc.decode(data).collect();
        assert_eq!(strict.len(), data.len());
        for (i, (s, l)) in strict.iter().zip(&lossy).enumerate() {
            match s {
                Ok(c) => assert_eq!(c, l, "{}: strict and lossy disagreed at {i}", enc.name()),
                Err(e) => {
                    assert_eq!(*l, char::REPLACEMENT_CHARACTER);
                    assert_eq!(e.offset, i, "{}: error offset is not the byte", enc.name());
                }
            }
        }

        let owned_lossy = enc.decode_to_string_lossy(data);
        assert_eq!(owned_lossy.chars().count(), data.len());
        assert!(owned_lossy.chars().eq(lossy.iter().copied()));

        // Round trip. A single-byte code page is a bijection between its
        // defined bytes and the characters they name, so when every byte
        // decodes, re-encoding must give the input back exactly. This is the
        // property that catches a duplicated or transposed table entry, which
        // is otherwise invisible: a wrong-but-defined character decodes fine
        // and only shows up as mojibake in someone's filename.
        match enc.decode_to_string(data) {
            Ok(s) => {
                assert_eq!(s.chars().count(), data.len());
                let back = enc
                    .encode_from_str(&s)
                    .unwrap_or_else(|e| panic!("{}: decoded text would not re-encode: {e:?}", enc.name()));
                assert_eq!(back, data, "{}: byte did not survive the round trip", enc.name());
            }
            Err(e) => {
                // Strict decoding fails only at a byte the page leaves
                // undefined, and must name it.
                assert!(e.offset < data.len());
                assert!(strict[e.offset].is_err(), "{}: reported a defined byte", enc.name());
            }
        }
    }
});

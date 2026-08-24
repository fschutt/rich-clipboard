//! RFC 2483 `text/uri-list` — `rclip_uri_list::parse`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_uri_list::{emit, parse, Entry, FileAction};

fuzz_target!(|data: &[u8]| {
    let Ok(list) = parse(data) else { return };

    // Drive every lazy accessor: the parse itself only validates UTF-8, so all
    // the interesting work happens in the iterators.
    let mut uris = Vec::new();
    for entry in list.entries() {
        match entry {
            Entry::Comment { text, offset } => {
                let _ = (text.len(), offset);
            }
            Entry::Uri(u) => {
                let _ = u.scheme();
                let _ = u.is_file();
                let _ = u.target();
                if let Some(f) = u.as_file() {
                    let _ = (f.host(), f.path(), f.is_local());
                }
                // Percent-decoding allocates from the input length; it must
                // never over-read the escape at the end of a truncated URI.
                let decoded = u.to_decoded_bytes();
                let _ = u.to_decoded_string();
                // The cheap allocation-free check and the decoder must agree
                // about what is well-formed. A `validate` that passes something
                // the decoder then chokes on is how a caller ends up trusting a
                // truncated escape.
                assert_eq!(
                    u.validate_percent_encoding().is_ok(),
                    decoded.is_ok(),
                    "validate_percent_encoding disagreed with the decoder on {:?}",
                    u.as_str()
                );
                uris.push(u.as_str());
            }
        }
    }
    let _ = list.validate_percent_encoding();
    let _ = list.first();

    // Round trip through the serializer. Lossy by design: `write_uri_list`
    // emits CRLF after every URI and drops comment lines and blank lines
    // entirely, so byte equality with the input is not the property. What must
    // hold is that the sequence of URIs is a fixed point.
    let blob = emit::write_uri_list(uris.iter().copied());
    let round = parse(&blob).expect("our own output must parse");
    let again: Vec<&str> = round.uris().map(|u| u.as_str()).collect();
    assert_eq!(again, uris);

    // ... and that a second pass changes nothing.
    assert_eq!(emit::write_uri_list(again.iter().copied()), blob);

    // The KDE flavor is a one-byte payload, so round-tripping it here is free.
    for action in [FileAction::Copy, FileAction::Cut] {
        assert_eq!(
            rclip_uri_list::convention::parse_kde_cut_selection(action.kde_payload()),
            action
        );
    }
});

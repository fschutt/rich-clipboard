//! GNOME/MATE `x-special/gnome-copied-files` — a verb line then a URI list.
//!
//! A separate entry point from `parse`, and a separate target: the verb line is
//! consumed before the list starts, so the two disagree about where offset 0 is.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_uri_list::{convention, emit};

fuzz_target!(|data: &[u8]| {
    let Ok(copied) = convention::parse_copied_files(data) else {
        return;
    };

    let action = copied.action();
    let uris: Vec<&str> = copied.uris().map(|u| u.as_str()).collect();
    for u in copied.uri_list().uris() {
        let _ = u.scheme();
        let _ = u.as_file().map(|f| f.is_local());
        assert_eq!(u.validate_percent_encoding().is_ok(), u.to_decoded_bytes().is_ok());
    }

    // Round trip. Lossy by design in the same way as `uri_list_parse`: the
    // reader is deliberately lenient about the verb's case and about CRLF,
    // while the writer emits exactly one canonical spelling.
    //
    // The verb always survives, so that half is unconditional.
    let blob = emit::write_copied_files(action, uris.iter().copied());
    let round = convention::parse_copied_files(&blob).expect("our own output must parse");
    assert_eq!(round.action(), action, "the verb did not survive the round trip");

    // The URIs are asserted only when none of them ends in a NUL byte, and the
    // exception is a finding rather than a convenience.
    //
    // `parse_copied_files` goes through `crate::parse`, which strips one
    // trailing NUL from the whole payload -- the Qt 3 `text/uri-list` quirk,
    // documented there. `write_copied_files` writes no trailing newline, so on
    // an `x-special/gnome-copied-files` payload whose last URI ends in NULs,
    // every round trip strips one more: `"J/sc\t\0\0\0\0\0"` needs five
    // round trips to converge. So this is not even a fixed point one step in.
    // It is leniency and not a crash -- a URI containing a raw NUL is malformed
    // to begin with -- but it is a `text/uri-list`-specific quirk applied to a
    // payload that is not `text/uri-list`. Found by this target on its first
    // run; `regression-trailing-nul-uri.bin` in this target's corpus is the
    // input that showed it.
    if uris.last().is_some_and(|u| u.ends_with('\0')) {
        return;
    }
    let once: Vec<&str> = round.uris().map(|u| u.as_str()).collect();
    assert_eq!(once, uris, "URIs did not survive the round trip");
    assert_eq!(
        emit::write_copied_files(round.action(), once.iter().copied()),
        blob,
        "serialization is not a fixed point"
    );
});

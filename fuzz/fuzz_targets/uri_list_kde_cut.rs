//! `application/x-kde-cutselection` — a one-byte payload, and infallible.
//!
//! No `Result` to check here: the function returns a `FileAction` for any input
//! at all. The property that matters is that it never panics on a short or
//! empty buffer, and that it only ever reads Cut from the exact byte KIO
//! writes — getting that backwards turns every KDE copy into a move.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_uri_list::{convention::parse_kde_cut_selection, FileAction};

fuzz_target!(|data: &[u8]| {
    let action = parse_kde_cut_selection(data);

    if action == FileAction::Cut {
        assert_eq!(
            data.first(),
            Some(&b'1'),
            "a payload that is not the cut byte was read as Cut"
        );
    }

    // Round trip: both actions must survive their own wire spelling.
    for a in [FileAction::Copy, FileAction::Cut] {
        assert_eq!(parse_kde_cut_selection(a.kde_payload()), a);
    }
});

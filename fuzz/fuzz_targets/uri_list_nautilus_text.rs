//! The legacy `x-special/nautilus-clipboard` payload, which arrives on the
//! *text* target and so has to be sniffed out of arbitrary pasted text.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_uri_list::convention::{
    is_nautilus_text_clipboard, parse_nautilus_text_clipboard, NAUTILUS_TEXT_MAGIC,
};

fuzz_target!(|data: &[u8]| {
    // The sniffer decides whether a plain `text/plain` paste gets reinterpreted
    // as a file *move*, so a false positive is the dangerous direction: it must
    // never claim a payload that does not literally start with the magic.
    let detected = is_nautilus_text_clipboard(data);
    assert_eq!(detected, data.starts_with(NAUTILUS_TEXT_MAGIC.as_bytes()));

    // Note the deliberate asymmetry the other way: `parse_` trims the magic
    // line and `crate::parse` skips a BOM, so `\u{feff}x-special/...` parses but
    // does not sniff. A false *negative* leaves the payload as plain text,
    // which is the safe outcome, so it is not asserted against here.
    let Ok(copied) = parse_nautilus_text_clipboard(data) else {
        return;
    };
    let _ = copied.action();
    for u in copied.uris() {
        let _ = u.scheme();
        let _ = u.target();
        assert_eq!(u.validate_percent_encoding().is_ok(), u.to_decoded_bytes().is_ok());
    }
});

//! The XML property list reader behind a `.webloc` — `rclip_webloc::xml`.
//!
//! A separate entry point from the binary reader, with a different failure
//! surface: a tag scanner over borrowed `&str` rather than an offset table.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_webloc::xml;

fuzz_target!(|data: &[u8]| {
    let detected = xml::detect(data);
    let Ok(doc) = xml::as_str(data) else {
        return;
    };

    // `as_str` is the gate `detect` is supposed to predict. A document that
    // decodes but was not detected simply never reaches the reader, which is
    // safe; the reverse -- detected but undecodable -- is handled by the
    // `Result` above.
    let _ = detected;

    // Only top-level pairs are yielded: a `URL` key one level down must not be
    // mistaken for the document's URL, which is what a crafted file would use
    // to redirect a reader that takes the first match.
    let mut pairs = 0usize;
    for pair in xml::Entries::new(doc) {
        let Ok((key, value)) = pair else { break };
        pairs += 1;
        assert!(pairs <= doc.len() + 1, "entry iterator did not advance");
        let _ = key.to_string_lossy();
        let _ = value.to_string_lossy();
        for c in key.chars() {
            let _ = c;
        }
        for c in value.chars() {
            let _ = c;
        }
    }
});

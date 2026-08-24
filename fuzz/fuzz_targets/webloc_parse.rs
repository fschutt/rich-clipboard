//! macOS `.webloc` / `.inetloc` — `rclip_webloc::Webloc::parse`, which
//! dispatches on a sniff between the XML and the binary plist readers.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_webloc::{Encoding, Webloc};

fuzz_target!(|data: &[u8]| {
    let detected = Webloc::detect(data);
    let parsed = Webloc::parse(data);

    // `parse` is documented as `detect` followed by the matching reader, so a
    // parse that succeeds where detect said "neither" would mean the dispatch
    // and the sniffer have drifted apart.
    if parsed.is_ok() {
        assert!(detected.is_some(), "parsed a file detect() did not recognise");
    }

    let Ok(loc) = parsed else { return };
    assert_eq!(Some(loc.encoding()), detected);
    match loc.encoding() {
        Encoding::Binary => assert!(data.starts_with(b"bplist00")),
        Encoding::Xml => {}
        // Added after this target was written: the pre-OS X form, a Macintosh
        // resource fork with a `url ` resource. It is sniffed *structurally*
        // rather than by a signature -- there is no magic to match, so
        // `detect` checks that the header's four offsets and the map's own two
        // agree with the buffer -- which makes it the only encoding here a
        // mutator can reach by arithmetic rather than by reproducing a string.
        // `webloc_rsrc` drives the fork reader directly.
        Encoding::ResourceFork => {
            assert!(
                !data.starts_with(b"bplist00"),
                "a binary plist was sniffed as a resource fork"
            );
            // The legacy form carries no title: Finder puts the name in the
            // filename and writes no `urln` resource.
            assert!(
                loc.url_name().is_none(),
                "a resource fork produced a URLName it cannot carry"
            );
        }
    }

    // `URL` is documented as always present -- parse fails without it.
    let url = loc.url();
    let _ = url.to_string_lossy();
    let _ = url.eq_str("https://example.com");
    for c in url.chars() {
        let _ = c;
    }
    if let Some(n) = loc.url_name() {
        let _ = n.to_string_lossy();
        for c in n.chars() {
            let _ = c;
        }
    }
});

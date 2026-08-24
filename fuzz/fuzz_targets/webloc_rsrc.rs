//! The legacy `.inetloc` form — `rclip_webloc::rsrc`, a Macintosh resource
//! fork.
//!
//! New code since the last sweep and the most index-heavy thing in the crate.
//! A resource fork is four offsets in a 16-byte header, a resource map with two
//! more offsets of its own, a type list whose count is `n - 1` on the wire, and
//! a reference list per type in which each entry carries a 24-bit data offset
//! and a 16-bit name offset — every one of them attacker-controlled, and every
//! one of them used to reach into the buffer.
//!
//! `Webloc::parse` can reach this, but only after the binary-plist and XML
//! sniffers have both declined, so driving `ResourceFork::parse` directly is
//! what gets the mutator's whole budget spent on the arithmetic.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_webloc::rsrc::{ResourceFork, HEADER_LEN, TYPE_DRAG, TYPE_TEXT, TYPE_URL};
use rclip_webloc::Webloc;

fuzz_target!(|data: &[u8]| {
    let detected = ResourceFork::detect(data);
    let Ok(fork) = ResourceFork::parse(data) else {
        // The converse of the assertion below is *not* a property, and the
        // crate says so outright: `detect` is the header check and nothing
        // more, because "is this a resource fork" and "is this resource fork
        // well formed" are different questions, and collapsing them would turn
        // every structural error inside the map into `BadMagic` -- telling a
        // caller the file is some other format when in fact it is this one,
        // broken. So a fork that detects and does not parse is the designed
        // behaviour; `regression-detect-without-parse.bin` is one.
        return;
    };
    assert!(detected, "parse() accepted a fork detect() rejected");

    // Both regions are slices of the input, so neither can outrun it.
    assert!(fork.data().len() <= data.len());
    assert!(fork.map().len() <= data.len());
    assert!(data.len() >= HEADER_LEN, "a fork shorter than its own header");

    // The type list. Its count comes off the wire as `n - 1`, which is the
    // classic off-by-one in this format, so it is bounded against the map
    // rather than trusted.
    let declared = fork.type_count();
    assert!(
        declared <= fork.map().len(),
        "more resource types than the map has bytes"
    );

    let mut types = 0usize;
    for ty in fork.types() {
        types += 1;
        assert!(types <= declared, "types() yielded more than type_count()");
        // Deliberately *not* asserted: that `ty.count` fits the map.
        // `ResourceFork::parse` validates the *type* count against the map and
        // stops there; a type's own resource count stays whatever the wire
        // said, and the walk below is what bounds it -- each reference entry is
        // read through the reader, so entry 60000 of a 30-byte map fails
        // cleanly rather than being pre-rejected. Bounding the walk instead of
        // the field is the right split, and it is the walk that is checked.
        let mut seen = 0usize;
        for res in ty.resources() {
            let Ok(res) = res else { break };
            seen += 1;
            assert!(seen <= ty.count, "more resources than the type declared");
            assert!(
                seen <= fork.map().len(),
                "the reference-list walk outran the map it lives in"
            );
            // The data is a length-prefixed slice of the data fork.
            assert!(
                res.data.len() <= fork.data().len(),
                "a resource outran the data fork"
            );
            if let Some(name) = res.name {
                assert!(name.len() <= fork.map().len(), "a name outran the map");
            }
            let _ = (res.id, res.attributes, res.is_compressed());
        }

        // `find_type` and the iterator have to agree that the code exists, or
        // a lookup silently misses a type the enumeration can see. Only the
        // code, though: a malformed map can list the same four-byte type twice,
        // and `find_type` is a first-match search, so its `count` legitimately
        // belongs to the *other* entry.
        let found = fork.find_type(ty.code).expect("types() yielded an unfindable type");
        assert_eq!(found.code, ty.code);
    }
    assert_eq!(types, declared, "types() stopped short of type_count()");

    for code in [TYPE_URL, TYPE_TEXT, TYPE_DRAG] {
        let by_iter = fork.resources(code).next().and_then(core::result::Result::ok);
        let first = fork.first_resource(code).and_then(core::result::Result::ok);
        assert_eq!(
            first.map(|r| (r.id, r.data)),
            by_iter.map(|r| (r.id, r.data)),
            "first_resource disagreed with resources()"
        );
    }

    // The location reader on top of it. Failing is normal — most forks have no
    // `url ` resource — but it must fail rather than panic, and when it
    // succeeds the URL has to be the resource's own bytes.
    if let Ok(loc) = Webloc::parse_resource_fork(data) {
        assert_eq!(loc.encoding(), rclip_webloc::Encoding::ResourceFork);
        assert!(loc.url_name().is_none(), "the legacy form has no URLName");
        let url = loc.url().to_string_lossy();
        let res = fork
            .first_resource(TYPE_URL)
            .expect("parsed a URL with no url resource")
            .expect("parsed a URL from an unreadable resource");
        assert_eq!(url.as_bytes(), res.data, "the URL was not the resource");
    }
});

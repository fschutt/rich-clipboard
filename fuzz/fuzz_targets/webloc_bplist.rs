//! The binary property list reader behind a `.webloc` —
//! `rclip_webloc::BinaryPlist::parse` and the object graph walk on top of it.
//!
//! A separate entry point from `Webloc::parse` and a separate target: the
//! object table is a set of offsets that reference each other, so a dictionary
//! whose value points back at the dictionary is how a crafted file spells
//! "recurse forever". The corpus carries a self-referential seed.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_webloc::{bplist::Object, BinaryPlist};

fuzz_target!(|data: &[u8]| {
    assert_eq!(BinaryPlist::detect(data), data.starts_with(b"bplist00"));

    let Ok(plist) = BinaryPlist::parse(data) else {
        return;
    };

    // Resolve every object the table can name, not just the reachable ones:
    // an unreachable object is still an offset the parser has to bound.
    let mut budget = data.len() + 1;
    let mut stack = vec![(plist.top_object(), 0u32)];
    while let Some((index, depth)) = stack.pop() {
        // A shared subtree can be visited more than once; the budget is what
        // keeps a fan-out bomb linear. This mirrors the node budget
        // `rclip_bookmark::Bookmark::validate` uses for the same attack shape.
        if budget == 0 {
            break;
        }
        budget -= 1;

        let Ok(obj) = plist.object(index, depth) else {
            continue;
        };
        match obj {
            Object::Str(t) => {
                let _ = t.to_string_lossy();
                for c in t.chars() {
                    let _ = c;
                }
            }
            Object::Dict {
                keys,
                values,
                count,
            } => {
                // `count` came off the wire. It must already have been checked
                // against the reference slices, or this loop is the hang.
                assert!(count <= data.len(), "dict count outran the file");
                for i in 0..count {
                    if let Ok(k) = plist.reference(keys, i) {
                        stack.push((k, depth + 1));
                    }
                    if let Ok(v) = plist.reference(values, i) {
                        stack.push((v, depth + 1));
                    }
                }
            }
            Object::Other => {}
        }
    }
});

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

    // The node budget, which is now the crate's rather than this target's:
    // `object` takes it by `&mut` so that *siblings share it*, because a
    // per-path counter resets on every branch and catches nothing. Depth alone
    // is not enough here and the crate has the measurement to prove it -- 223
    // bytes of dictionaries nesting only nine levels cost 40 million
    // resolutions -- so this is the guard that turns a fan-out bomb into an
    // error, and driving it is most of the point of this target.
    let start = plist.budget();
    let mut budget = start;
    let mut stack = vec![(plist.top_object(), 0u32)];
    let mut visits = 0usize;
    while let Some((index, depth)) = stack.pop() {
        visits += 1;
        // The queue is this target's own structure and the crate's budget says
        // nothing about it, so it gets its own cap rather than an assertion.
        if visits > 4 * start + 16 {
            break;
        }
        if budget == 0 {
            // Nothing can be resolved on an exhausted budget. Documented as
            // `ErrorKind::TooLarge`, though an out-of-range index or a
            // depth-limited hop is checked first and reports itself, so what is
            // asserted is the part that holds unconditionally.
            assert!(
                plist.object(index, depth, &mut budget).is_err(),
                "an exhausted budget still resolved an object"
            );
            break;
        }
        let before = budget;
        let resolved = plist.object(index, depth, &mut budget);
        assert!(budget <= before, "the node budget grew");
        let Ok(obj) = resolved else {
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

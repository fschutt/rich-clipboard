//! `CIDA` (`CFSTR_SHELLIDLIST`) — `rclip_idlist::Cida::parse`.
//!
//! A separate entry point from a bare `ITEMIDLIST` and a separate target: a
//! CIDA is a `cidl` count followed by `cidl + 1` `aoffset` entries, every one
//! of which is an attacker-controlled index into the same blob. The plan calls
//! `CIDA.aoffset` out by name.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_idlist::Cida;

fuzz_target!(|data: &[u8]| {
    let Ok(cida) = Cida::parse(data) else { return };

    // The parent list plus one child per declared item. Each list walk is
    // itself bounded; the point here is that the offsets that locate them were
    // checked against the buffer at parse time.
    if let Ok(parent) = cida.parent() {
        for item in parent {
            let Ok(item) = item else { break };
            let _ = item.parse().display_name();
        }
    }

    let mut seen = 0usize;
    for child in cida.children() {
        let Ok(child) = child else { break };
        seen += 1;
        for item in child {
            let Ok(item) = item else { break };
            let _ = item.parse().display_name();
        }
        // A CIDA with a huge `cidl` must have been rejected at parse time, not
        // here: check_count is what stands between this loop and a hang.
        assert!(seen <= data.len(), "children iterator outran the buffer");
    }

    for i in 0..seen.min(64) {
        let _ = cida.offset(i);
        let _ = cida.child(i);
    }
});

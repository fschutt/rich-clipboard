//! A bare `ITEMIDLIST` / PIDL — `rclip_idlist::ItemIdList` plus
//! `ShellItem::parse` for every item in it.
//!
//! The list is a chain of `cb`-prefixed items where `cb` comes straight off the
//! wire: `cb == 0` and `cb == 1` are the two shapes that make a naive walker
//! loop forever, and the corpus carries a seed for each.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_idlist::{display_path, ExtensionBlock, ItemIdList, ItemIdListBuilder, ShellItem};

fuzz_target!(|data: &[u8]| {
    // Owned copies, not borrows: the round-trip below re-parses a buffer this
    // function owns, and comparing the two needs both alive at once.
    let mut bodies: Vec<Vec<u8>> = Vec::new();

    // The walk must terminate: every step advances by at least MIN_ITEM_SIZE.
    for item in ItemIdList::new(data) {
        let Ok(item) = item else { break };
        let _ = item.class();
        let parsed = item.parse();
        bodies.push(parsed.as_bytes().to_vec());
        let _ = parsed.class();
        let _ = parsed.display_name().map(|n| n.to_string_lossy());

        match parsed {
            ShellItem::RootFolder(r) => {
                let _ = (r.sort_index, r.guid.well_known_name());
            }
            ShellItem::Volume(v) => {
                let _ = v.name.map(|n| n.to_string_lossy());
                let _ = v.guid;
            }
            ShellItem::FileEntry(f) => {
                let _ = f.primary_name.to_string_lossy();
                let _ = f.long_name.map(|n| n.to_string_lossy());
                let _ = f.localized_name.map(|n| n.to_string_lossy());
                let _ = (f.file_size, f.modified, f.attributes);
            }
            ShellItem::NetworkLocation(n) => {
                let _ = n.location.to_string_lossy();
                let _ = n.description.map(|s| s.to_string_lossy());
                let _ = n.comment.map(|s| s.to_string_lossy());
            }
            ShellItem::Uri(u) => {
                let _ = u.uri.map(|s| s.to_string_lossy());
            }
            _ => {}
        }

        // Extension blocks are a second, independently attacker-controlled
        // chain hanging off the item body.
        for block in ExtensionBlock::walk(parsed.as_bytes()) {
            let _ = (block.offset, block.size, block.version, block.signature);
            if let Some(f) = block.as_file_entry() {
                let _ = (f.mft_entry(), f.mft_sequence(), f.created, f.accessed);
                let _ = f.long_name.map(|n| n.to_string_lossy());
            }
        }
    }

    // `try_len` walks the same chain and must terminate on the same input.
    // It reports the structural error the iterator swallowed, so the counts
    // only have to agree when there was no error.
    if let Ok(n) = ItemIdList::new(data).try_len() {
        assert_eq!(n, bodies.len());
    }
    let _ = display_path(ItemIdList::new(data), "\\");

    // Round trip. Lossy by design: the builder always emits a canonical
    // terminator and drops whatever trailing slack the input had, so byte
    // equality only holds for an already-canonical list. The item bodies are
    // the value, and they must come back identical.
    let mut b = ItemIdListBuilder::new();
    let mut pushed = 0usize;
    for body in &bodies {
        if !b.push_raw(body) {
            // `cb` is a u16; a body that does not fit is refused rather than
            // truncated, which is the correct answer and not a round-trip
            // failure.
            break;
        }
        pushed += 1;
    }
    let bytes = b.finish();
    let round: Vec<Vec<u8>> = ItemIdList::new(&bytes)
        .map_while(Result::ok)
        .map(|i| i.parse().as_bytes().to_vec())
        .collect();
    assert_eq!(round.len(), pushed, "builder output lost items");
    assert_eq!(round.as_slice(), &bodies[..pushed], "item body changed");
});

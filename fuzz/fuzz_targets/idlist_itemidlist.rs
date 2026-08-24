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

            // The signature-recognised items. New code since the last sweep,
            // and reached by a *different* dispatch from everything above it:
            // several shell extensions write a class byte of `0x00` and
            // identify themselves with a 32-bit signature at `abID[4..8]`
            // instead, so `signature::recognise` runs before the class byte is
            // even looked at. Leaving them in a `_ => {}` arm meant the
            // enumeration reached them and none of their accessors did.
            ShellItem::MtpVolume(v) => {
                let _ = v.name.map(|n| n.to_string_lossy());
                let _ = v.identifier.map(|n| n.to_string_lossy());
                let _ = v.file_system.map(|n| n.to_string_lossy());
                assert!(v.raw.len() <= data.len());
            }
            ShellItem::MtpFileEntry(f) => {
                let _ = f.name.map(|n| n.to_string_lossy());
                let _ = f.identifier.map(|n| n.to_string_lossy());
                let _ = (f.modified, f.created, f.content_type);
                assert!(f.raw.len() <= data.len());
            }
            ShellItem::UsersPropertyView(v) => {
                assert!(
                    v.identifier.len() <= v.raw.len(),
                    "the identifier outran the item it came from"
                );
                assert!(
                    v.property_store.len() <= v.raw.len(),
                    "the property store outran the item it came from"
                );
                let _ = (v.signature, v.known_folder_id);
            }
            ShellItem::CompressedFolder(c) => {
                let _ = c.name.map(|n| n.to_string_lossy());
                let _ = (c.variant, c.uncompressed_size, c.compressed_size);
                let _ = c.compression_method;
            }
            ShellItem::ControlPanel(c) => {
                let _ = (c.identifier, c.name());
            }
            // The one that could recurse. A delegate folder wraps another
            // shell item, and `inner_item` is documented as re-dispatching
            // *without* the delegate probe precisely so a nested delegate
            // cannot re-enter -- which is the shape a hostile PIDL would use.
            ShellItem::DelegateFolder(d) => {
                assert!(
                    d.inner.len() < d.raw.len(),
                    "the inner item did not shrink, so nesting could not terminate"
                );
                let inner = d.inner_item();
                assert!(
                    !matches!(inner, ShellItem::DelegateFolder(_)),
                    "a delegate folder re-entered the delegate path"
                );
                let _ = inner.display_name().map(|n| n.to_string_lossy());
                let _ = d.folder_id;
            }
            // `Empty`, `Unknown`, and -- `ShellItem` being `#[non_exhaustive]`
            // -- any variant added after this build. None of them has an
            // accessor to drive.
            _ => {}
        }

        // The signature dispatch on its own, both ways. `parse_no_delegate` is
        // the entry point `inner_item` uses, so it is reachable from a hostile
        // PIDL and has to stand up to one on its own.
        let _ = rclip_idlist::signature::recognise(parsed.as_bytes(), true);
        let _ = rclip_idlist::signature::recognise(parsed.as_bytes(), false);
        let _ = ShellItem::parse_no_delegate(parsed.as_bytes())
            .display_name()
            .map(|n| n.to_string_lossy());

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

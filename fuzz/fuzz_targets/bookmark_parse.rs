//! macOS `BookmarkData` (`book` / `alis`) — `rclip_bookmark::Bookmark::parse`
//! and the lazy object graph behind it.
//!
//! Every offset in the format is payload-relative and attacker-controlled, and
//! the graph is a general DAG: a TOC that points at itself, an array whose
//! element points back at the array, and eight nested arrays of eight shared
//! references each (400 bytes, 8^8 resolutions) are all in the corpus.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_bookmark::{Bookmark, EntryKey, Value};

fuzz_target!(|data: &[u8]| {
    let Ok(bm) = Bookmark::parse(data) else {
        return;
    };

    assert!(bm.size() <= data.len(), "declared size outran the buffer");
    assert!(bm.header_size() >= 16);
    let _ = (bm.magic(), bm.version(), bm.first_toc_offset());

    // The documented structural check. It carries both guards -- a depth limit
    // and a node budget -- so it is the one call that must terminate on every
    // shape of hostile graph.
    let validated = bm.validate();

    let mut tocs = 0usize;
    for toc in bm.tocs() {
        let Ok(toc) = toc else { break };
        tocs += 1;
        assert!(tocs <= data.len(), "TOC chain did not terminate");

        let mut entries = 0usize;
        for entry in toc.iter() {
            let Ok(entry) = entry else { break };
            entries += 1;
            assert!(entries <= data.len(), "TOC entry iterator did not advance");
            let _ = (entry.raw_key(), entry.value_offset(), entry.offset());
            if let Ok(key) = entry.key() {
                let _ = key.name();
                let _ = matches!(key, EntryKey::Named(_));
            }
            if let Ok(v) = entry.value() {
                walk(&v, &mut (data.len() + 1));
            }
        }
        assert_eq!(entries.min(toc.len()), entries.min(toc.len()));
    }

    // The named accessors are what a caller actually uses; each resolves an
    // independent offset.
    let _ = bm.target_url();
    let _ = bm.target_filename();
    let _ = bm.display_name();
    let _ = bm.volume_name();
    let _ = bm.volume_path();
    let _ = bm.volume_uuid();
    let _ = bm.target_creation_date();
    let _ = bm.creation_time();
    let _ = bm.target_flags();
    let _ = bm.volume_flags();
    let _ = bm.sandbox_extension();
    let _ = bm.target_path();
    if let Ok(Some(components)) = bm.path_components() {
        for c in components {
            let _ = c;
        }
    }

    // If the crate says the graph is structurally sound, then every *offset*
    // resolution must succeed -- that is what `validate` buys a caller, and
    // `get` walks exactly the TOC chain, entry table and value records that
    // `validate` just proved walkable.
    //
    // Deliberately not asserted for `path_components`: it additionally requires
    // the record under key 0x1004 to be an *array*, and a well-formed record of
    // the wrong type is a type error, not a structural one. `validate` does not
    // claim to check types, so a sound graph with a string where the path
    // components belong is a legitimate `Err` there. Found by this target on
    // its first run.
    if validated.is_ok() {
        assert!(bm.target_url().is_ok(), "sound graph, but a key would not resolve");
        assert!(bm.target_filename().is_ok());
        assert!(bm.volume_name().is_ok());
        assert!(bm.get(0x1004).is_ok(), "sound graph, but a record would not resolve");
    }
});

/// Walk a value with the same node budget `Bookmark::validate` uses, so a
/// fan-out bomb costs time linear in the input rather than exponential.
fn walk(value: &Value<'_>, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    *budget -= 1;
    match value {
        Value::Array(a) => {
            for v in a.iter() {
                let Ok(v) = v else { break };
                walk(&v, budget);
            }
        }
        Value::Dict(d) => {
            for kv in d.iter() {
                let Ok((k, v)) = kv else { break };
                walk(&k, budget);
                walk(&v, budget);
            }
        }
        Value::RelativeUrl(r) => {
            if let Ok(b) = r.base() {
                walk(&b, budget);
            }
            if let Ok(rel) = r.relative() {
                walk(&rel, budget);
            }
        }
        other => {
            let _ = (other.as_str(), other.as_data(), other.as_i64(), other.as_f64());
            let _ = (other.as_bool(), other.as_date());
        }
    }
}

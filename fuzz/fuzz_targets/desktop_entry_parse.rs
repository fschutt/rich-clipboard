//! freedesktop `.desktop` entries — `rclip_desktop_entry::parse`.
//!
//! The escape decoder and the `Exec` field-code splitter are the parts worth
//! fuzzing: both walk a borrowed `&str` looking for a following character, and
//! both are reachable from a value that ends mid-escape.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_desktop_entry::{parse, ExecPiece, Locale};

fuzz_target!(|data: &[u8]| {
    let Ok(f) = parse(data) else { return };

    let mut groups = 0usize;
    for g in f.groups() {
        groups += 1;
        assert!(groups <= data.len() + 1, "group iterator did not advance");
        let _ = g.name();

        let mut entries = 0usize;
        for e in g.entries() {
            entries += 1;
            assert!(entries <= data.len() + 1, "entry iterator did not advance");
            // §3.3: keys are `A-Za-z0-9-` and `parse` validates that, so a key
            // with anything else in it means the validator and the splitter
            // disagree about where the key ends.
            assert!(
                e.key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "entry key escaped validation: {:?}",
                e.key
            );
            if let Some(loc) = e.locale {
                let _ = Locale::parse(loc);
            }
            // Unescaping is the interesting path: `\` at end of value, `\u`
            // with nothing after it, `\` followed by a multi-byte character.
            for c in e.value.chars() {
                let _ = c;
            }
            let _ = e.value.to_unescaped();
            let _ = e.value.to_unescaped_lossy();
            let _ = e.value.as_bool();
            let _ = e.value.as_f64();
            let _ = e.value.eq_str("true");
        }

        let _ = g.boolean("Terminal");
        let _ = g.numeric("Version");
        if let Some(items) = g.list("Actions") {
            for item in items {
                let _ = item.to_unescaped_lossy();
            }
        }
        if let Some(exec) = g.exec() {
            let _ = exec.validate();
            let _ = exec.program();
            for arg in exec.args() {
                let Ok(arg) = arg else { break };
                let _ = arg.as_field();
                for piece in arg.pieces() {
                    match piece {
                        // A field code is returned, never expanded: expanding
                        // `%f` here would be this crate performing an action.
                        Ok(ExecPiece::Field(_) | ExecPiece::Char(_)) | Err(_) => {}
                        _ => {}
                    }
                }
            }
        }
    }

    let _ = f.entry_type();
    let _ = f.url();
    // Explicitly *not* resolved, launched or touched in any way: this crate
    // returns data. See CONVENTIONS.md rule 6, which names `.desktop` directly.
    let _ = f.target();
    let _ = f.name(None);
    let _ = f.name(Locale::parse("de_DE.UTF-8").as_ref());
    if let Some(ids) = f.action_ids() {
        for id in ids {
            if let Ok(id) = id.to_unescaped() {
                let _ = f.action(&id);
            }
        }
    }
});

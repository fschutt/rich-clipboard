//! macOS: `NSPasteboard`.
//!
//! **Verified** — built and run against a real pasteboard on macOS 15.
//!
//! Two things about `NSPasteboard` are easy to get wrong and both matter for a
//! capture tool:
//!
//! * **A pasteboard holds *items*, not formats.** `-[NSPasteboard types]` is
//!   the union of every item's types and `-[NSPasteboard dataForType:]` only
//!   ever reaches the *first* item that offers that type. Copy three files in
//!   Finder and the pasteboard has three items each carrying one
//!   `public.file-url`; the pasteboard-level API shows one URL and silently
//!   drops the other two. So when there is more than one item this walks
//!   `-pasteboardItems` and dumps each item separately.
//! * **Types are promises.** `dataForType:` can return nil for a type that is
//!   genuinely on offer, because the owning application declared it lazily and
//!   then declined (or exited) when asked. That is reported, not hidden.
//!
//! Everything here is a safe call: the `objc2-app-kit` bindings mark
//! `generalPasteboard`, `types`, `pasteboardItems` and `dataForType:` safe, so
//! this backend contains no `unsafe` at all.

use objc2_app_kit::NSPasteboard;

use crate::capture::{Body, Capture, Offered, Result, Selection};

pub fn capture(_selection: Selection) -> Result<Capture> {
    // `Selection::Primary` is rejected in `main` — macOS has one pasteboard
    // per name and no select-to-copy selection at all.
    let pasteboard = NSPasteboard::generalPasteboard();

    let items = pasteboard.pasteboardItems();
    let item_count = items.as_ref().map_or(0, |a| a.len());

    let offered = if item_count > 1 {
        let mut offered = Vec::new();
        for (index, item) in items.iter().flat_map(|a| a.iter()).enumerate() {
            for ty in item.types().iter() {
                let native = ty.to_string();
                let body = match item.dataForType(&ty) {
                    Some(data) => Body::Bytes(data.to_vec()),
                    None => Body::Skipped(format!(
                        "-[NSPasteboardItem dataForType:@\"{native}\"] returned nil"
                    )),
                };
                offered.push(Offered {
                    native,
                    item: Some(index),
                    body,
                    detail: None,
                });
            }
        }
        offered
    } else {
        // One item (or an empty pasteboard): the pasteboard-level API sees
        // everything, and it is the API every real application uses, so a
        // capture through it is the more faithful one.
        let types = pasteboard.types();
        types
            .iter()
            .flat_map(|a| a.iter())
            .map(|ty| {
                let native = ty.to_string();
                let body = match pasteboard.dataForType(&ty) {
                    Some(data) => Body::Bytes(data.to_vec()),
                    None => Body::Skipped(format!(
                        "-[NSPasteboard dataForType:@\"{native}\"] returned nil \
                         (declared but not provided)"
                    )),
                };
                Offered {
                    native,
                    item: None,
                    body,
                    detail: None,
                }
            })
            .collect()
    };

    Ok(Capture {
        platform: rclip_core::Platform::MacOs,
        source: if item_count > 1 {
            format!("the macOS general pasteboard ({item_count} items)")
        } else {
            "the macOS general pasteboard".to_owned()
        },
        offered,
    })
}

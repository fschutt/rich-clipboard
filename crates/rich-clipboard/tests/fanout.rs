//! The write-side table itself.
//!
//! No format features needed: the table is data, and it is the half of the
//! crate that a `no_std` build still gets.

use rclip_core::{Flavor, Platform};
use rich_clipboard::{native_name, write_plan, Fidelity, ItemKind};

const PLATFORMS: [Platform; 3] = [Platform::Windows, Platform::MacOs, Platform::Unix];

const KINDS: [ItemKind; 10] = [
    ItemKind::Text,
    ItemKind::RichText,
    ItemKind::Html,
    ItemKind::Image,
    ItemKind::Files,
    ItemKind::PromisedFiles,
    ItemKind::Link,
    ItemKind::Shortcut,
    ItemKind::ShellItems,
    ItemKind::Unknown,
];

fn flavors(kind: ItemKind, platform: Platform) -> Vec<Flavor<'static>> {
    write_plan(kind, platform)
        .iter()
        .map(|w| w.flavor)
        .collect()
}

fn natives(kind: ItemKind, platform: Platform) -> Vec<&'static str> {
    write_plan(kind, platform)
        .iter()
        .filter_map(|w| native_name(w.flavor, platform))
        .collect()
}

#[test]
fn styled_text_fans_out_to_three_flavors_on_windows() {
    // The whole point of the crate in one assertion: publish styled text and
    // Word gets RTF, Chrome gets HTML, Notepad gets characters, and nobody has
    // to negotiate.
    assert_eq!(
        flavors(ItemKind::RichText, Platform::Windows),
        [Flavor::Rtf, Flavor::Html, Flavor::PlainText]
    );
    assert_eq!(
        natives(ItemKind::RichText, Platform::Windows),
        ["Rich Text Format", "HTML Format", "CF_UNICODETEXT"]
    );
}

#[test]
fn styled_text_is_rtf_plus_plain_on_macos() {
    // No `public.html`: Pages, TextEdit, Mail and Notes all speak RTF and
    // several speak no HTML at all.
    assert_eq!(
        natives(ItemKind::RichText, Platform::MacOs),
        ["public.rtf", "public.utf8-plain-text"]
    );
}

#[test]
fn styled_text_is_html_plus_plain_on_unix() {
    assert_eq!(
        natives(ItemKind::RichText, Platform::Unix),
        ["text/html", "text/plain;charset=utf-8"]
    );
}

#[test]
fn a_linux_file_copy_offers_all_three_conventions() {
    // None of them reads the others', so a source that publishes one and not
    // the rest has its cut silently read as a copy by two thirds of the
    // desktop.
    assert_eq!(
        natives(ItemKind::Files, Platform::Unix),
        [
            "text/uri-list",
            "x-special/gnome-copied-files",
            "x-special/mate-copied-files",
            "application/x-kde-cutselection",
        ]
    );
}

#[test]
fn a_windows_file_copy_carries_its_drop_effect() {
    assert_eq!(
        natives(ItemKind::Files, Platform::Windows),
        ["CF_HDROP", "Preferred DropEffect"]
    );
    let plan = write_plan(ItemKind::Files, Platform::Windows);
    assert_eq!(plan[1].fidelity, Fidelity::Sidecar);
}

#[test]
fn the_write_table_disagrees_with_read_rank_about_images_on_purpose() {
    // Reading prefers PNG: it has an unambiguous alpha convention and
    // CF_DIBV5 does not. Writing leads with CF_DIBV5 anyway, because Paint and
    // a long tail of Win32 applications read DIB and nothing else. Two
    // questions, two answers — which is why the write side needs its own table
    // rather than reusing `read_rank` reversed.
    assert!(Flavor::Png.read_rank() < Flavor::DibV5.read_rank());
    assert_eq!(
        flavors(ItemKind::Image, Platform::Windows)[0],
        Flavor::DibV5
    );
}

#[test]
fn no_bmp_is_offered_on_unix() {
    // `image/bmp` on X11 or Wayland means a BMP *file*, with the 14-byte
    // BITMAPFILEHEADER that CF_DIB omits by definition. Advertising it would
    // hand gdk-pixbuf bytes it cannot open.
    assert_eq!(natives(ItemKind::Image, Platform::Unix), ["image/png"]);
}

#[test]
fn the_unpublishable_kinds_say_so_rather_than_inventing_a_flavor() {
    for platform in PLATFORMS {
        assert!(write_plan(ItemKind::Shortcut, platform).is_empty());
        assert!(write_plan(ItemKind::ShellItems, platform).is_empty());
        // `Unknown` is republished verbatim by `encode`, so it is deliberately
        // not in the table: its flavor is not known until run time.
        assert!(write_plan(ItemKind::Unknown, platform).is_empty());
    }
    // Promised files are a Windows byte format and a protocol everywhere else.
    assert!(!write_plan(ItemKind::PromisedFiles, Platform::Windows).is_empty());
    assert!(write_plan(ItemKind::PromisedFiles, Platform::MacOs).is_empty());
    assert!(write_plan(ItemKind::PromisedFiles, Platform::Unix).is_empty());
}

#[test]
fn every_planned_flavor_has_a_name_on_its_own_platform() {
    // A plan entry the transport cannot name is an entry that silently
    // disappears at publish time. This is the invariant that catches a flavor
    // added to a table without a registry entry to go with it.
    for kind in KINDS {
        for platform in PLATFORMS {
            for entry in write_plan(kind, platform) {
                assert!(
                    native_name(entry.flavor, platform).is_some(),
                    "{kind:?} on {platform:?} plans {:?}, which has no identifier there",
                    entry.flavor
                );
            }
        }
    }
}

#[test]
fn every_plan_leads_with_a_full_fidelity_flavor() {
    // "Best first" is the contract a transport relies on when it publishes in
    // order, so the first entry must never be a lossy or metadata-only one.
    for kind in KINDS {
        for platform in PLATFORMS {
            if let Some(first) = write_plan(kind, platform).first() {
                assert_eq!(
                    first.fidelity,
                    Fidelity::Full,
                    "{kind:?} on {platform:?} leads with {:?}",
                    first.fidelity
                );
            }
        }
    }
}

#[test]
fn a_lossy_or_sidecar_entry_always_says_what_it_costs() {
    for kind in KINDS {
        for platform in PLATFORMS {
            for entry in write_plan(kind, platform) {
                match entry.fidelity {
                    Fidelity::Full => assert!(entry.note.is_empty()),
                    Fidelity::Lossy | Fidelity::Sidecar => assert!(
                        !entry.note.is_empty(),
                        "{kind:?}/{platform:?}/{:?} loses something and does not say what",
                        entry.flavor
                    ),
                }
            }
        }
    }
}

#[test]
fn a_plan_never_lists_the_same_flavor_twice() {
    for kind in KINDS {
        for platform in PLATFORMS {
            // O(n^2), and n is at most four. `Flavor` is `Hash + Eq` but not
            // `Ord`, and sorting by `read_rank` would collide unrelated
            // flavors that happen to share a rank.
            let seen = flavors(kind, platform);
            for (i, a) in seen.iter().enumerate() {
                for b in &seen[i + 1..] {
                    assert_ne!(a, b, "{kind:?} on {platform:?} repeats {a:?}");
                }
            }
        }
    }
}

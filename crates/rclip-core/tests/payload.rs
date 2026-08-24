//! A clipboard read is a *set* of encodings, and picking among them is the
//! consumer's call. These tests pin the picking rules.

#![cfg(feature = "alloc")]

use rclip_core::{ClipboardItem, ClipboardPayload, Flavor, Platform};

/// What a browser actually puts on a macOS pasteboard when you copy a table.
fn browser_copy() -> ClipboardPayload {
    ClipboardPayload::new(Platform::MacOs)
        .with("public.utf8-plain-text", b"Item 1\tItem 2".to_vec())
        .with("public.html", b"<table><tr><td>Item 1".to_vec())
        .with("public.rtf", b"{\\rtf1\\ansi Item 1}".to_vec())
}

#[test]
fn resolves_native_identifiers_per_platform() {
    let mac = ClipboardPayload::new(Platform::MacOs).with("public.html", b"x".to_vec());
    assert_eq!(mac.items()[0].flavor(Platform::MacOs), Flavor::Html);

    let win = ClipboardPayload::new(Platform::Windows).with("HTML Format", b"x".to_vec());
    assert_eq!(win.items()[0].flavor(Platform::Windows), Flavor::Html);

    let unix = ClipboardPayload::new(Platform::Unix).with("text/html", b"x".to_vec());
    assert_eq!(unix.items()[0].flavor(Platform::Unix), Flavor::Html);
}

#[test]
fn the_same_string_means_different_things_per_platform() {
    // "text/html" is a MIME type, not a UTI. Resolving it as one must not
    // silently succeed — that is how a Unix blob gets decoded as a macOS one
    // and produces garbage rather than an error.
    let p = ClipboardPayload::new(Platform::MacOs).with("text/html", b"x".to_vec());
    assert_eq!(
        p.items()[0].flavor(Platform::MacOs),
        Flavor::Other("text/html"),
        "a MIME type is not a UTI and must not resolve as one"
    );
}

#[test]
fn best_prefers_rich_over_plain() {
    let p = browser_copy();
    let best = p.best().expect("three items on offer");
    assert_eq!(
        best.flavor(Platform::MacOs),
        Flavor::Rtf,
        "plain text is derivable from RTF, so RTF must win"
    );
}

#[test]
fn best_skips_metadata_however_it_sorts() {
    // Preferred DropEffect outranks plain text numerically but is never what a
    // paste wanted.
    let p = ClipboardPayload::new(Platform::Windows)
        .with("Preferred DropEffect", vec![2, 0, 0, 0])
        .with("CF_UNICODETEXT", b"t\0e\0x\0t\0".to_vec());
    let best = p.best().expect("two items");
    assert_eq!(best.flavor(Platform::Windows), Flavor::PlainText);
}

#[test]
fn get_finds_a_specific_flavor_and_reports_absence() {
    let p = browser_copy();
    assert_eq!(
        p.get(Flavor::Html).map(|i| i.bytes.as_slice()),
        Some(&b"<table><tr><td>Item 1"[..])
    );
    assert!(p.get(Flavor::Png).is_none(), "nothing offered an image");
}

#[test]
fn first_listing_wins_for_a_duplicated_flavor() {
    let p = ClipboardPayload::new(Platform::Unix)
        .with("text/html", b"first".to_vec())
        .with("text/html", b"second".to_vec());
    assert_eq!(
        p.get(Flavor::Html).unwrap().bytes,
        b"first".to_vec(),
        "a source advertising a flavor twice is malformed; the first is what it meant"
    );
}

#[test]
fn unknown_identifiers_round_trip_verbatim() {
    // A private or vendor format must survive the round trip so it can be
    // handed straight back to the OS on write.
    let native = "application/x-acme-internal-v3";
    let p = ClipboardPayload::new(Platform::Unix).with(native, b"opaque".to_vec());
    let item = &p.items()[0];
    assert_eq!(item.flavor(Platform::Unix), Flavor::Other(native));
    assert_eq!(
        item.native, native,
        "the native name must not be normalised away"
    );
}

#[test]
fn empty_payload_has_no_best() {
    let p = ClipboardPayload::new(Platform::MacOs);
    assert!(p.is_empty());
    assert!(p.best().is_none());
    assert!(p.get(Flavor::PlainText).is_none());
}

#[test]
fn a_payload_of_only_metadata_has_no_best() {
    let p = ClipboardPayload::new(Platform::Windows).with("Preferred DropEffect", vec![1, 0, 0, 0]);
    assert!(!p.is_empty(), "the item is there");
    assert!(p.best().is_none(), "but none of it is content");
}

#[test]
fn flavors_iterates_in_source_order() {
    let p = browser_copy();
    let got: Vec<_> = p.flavors().collect();
    assert_eq!(got, vec![Flavor::PlainText, Flavor::Html, Flavor::Rtf]);
}

#[test]
fn push_and_builder_agree() {
    let mut a = ClipboardPayload::new(Platform::Unix);
    a.push(ClipboardItem::new("text/plain", b"x".to_vec()));
    let b = ClipboardPayload::new(Platform::Unix).with("text/plain", b"x".to_vec());
    assert_eq!(a, b);
}

/// What a three-file Finder copy actually looks like: three *items*, each
/// offering the same flavor, rather than one item offering it three times.
fn finder_three_file_copy() -> ClipboardPayload {
    ClipboardPayload::new(Platform::MacOs)
        .with_in(0, "public.file-url", b"file:///tmp/a.txt".to_vec())
        .with_in(0, "public.utf8-plain-text", b"a.txt".to_vec())
        .with_in(1, "public.file-url", b"file:///tmp/b.txt".to_vec())
        .with_in(1, "public.utf8-plain-text", b"b.txt".to_vec())
        .with_in(2, "public.file-url", b"file:///tmp/c.txt".to_vec())
}

#[test]
fn taking_only_the_first_match_loses_a_multi_file_selection() {
    let p = finder_three_file_copy();
    // `get` is first-match, which is right for "give me the HTML" and wrong
    // for a file list. This is the shape of the bug in -[NSPasteboard
    // dataForType:], which reaches only the first item offering a type.
    assert_eq!(
        p.get(Flavor::FileList).map(|i| i.bytes.as_slice()),
        Some(&b"file:///tmp/a.txt"[..])
    );
    // `all` is what a file list needs.
    let urls: Vec<_> = p
        .all(Flavor::FileList)
        .map(|i| i.bytes.as_slice())
        .collect();
    assert_eq!(
        urls,
        vec![
            &b"file:///tmp/a.txt"[..],
            &b"file:///tmp/b.txt"[..],
            &b"file:///tmp/c.txt"[..]
        ],
        "a three-file copy must come back as three files"
    );
}

#[test]
fn items_group_by_index() {
    let p = finder_three_file_copy();
    assert_eq!(p.item_count(), 3);
    assert_eq!(p.group(0).count(), 2, "item 0 offers a URL and a name");
    assert_eq!(p.group(2).count(), 1, "item 2 offers only a URL");
    assert_eq!(
        p.group(9).count(),
        0,
        "an item that does not exist is empty, not a panic"
    );
    assert_eq!(
        p.group(1)
            .find(|i| i.flavor(Platform::MacOs) == Flavor::PlainText)
            .map(|i| i.bytes.as_slice()),
        Some(&b"b.txt"[..])
    );
}

#[test]
fn a_single_item_payload_still_reads_as_one_item() {
    // Every platform but macOS has no notion of items, so the common case must
    // not have to think about them.
    let p = browser_copy();
    assert_eq!(p.item_count(), 1);
    assert_eq!(p.group(0).count(), 3);
    assert_eq!(
        p.items()[0].item,
        0,
        "new() puts a representation in item 0"
    );
}

#[test]
fn an_empty_payload_has_no_items() {
    let p = ClipboardPayload::new(Platform::MacOs);
    assert_eq!(p.item_count(), 0, "not 1 — there is nothing there");
}

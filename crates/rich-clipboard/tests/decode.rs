//! Decoding: which flavor wins, what it becomes, and what happens when nothing
//! this build understands is on offer.

use rclip_core::{ClipboardItem, ClipboardPayload, Platform};
use rich_clipboard::{decode, decode_payload, Error, RichItem};

#[allow(dead_code)]
fn fixture(krate: &str, name: &str) -> Vec<u8> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/");
    std::fs::read(format!("{root}{krate}/{name}")).expect("fixture")
}

// ---------------------------------------------------------------------------
// Flavor preference
// ---------------------------------------------------------------------------

#[test]
fn plain_text_decodes_on_every_platform_without_any_format_feature() {
    let win = ClipboardPayload::new(Platform::Windows).with("CF_UNICODETEXT", &b"h\0i\0\0\0"[..]);
    assert_eq!(decode_payload(&win).unwrap(), RichItem::Text("hi".into()));

    let mac = ClipboardPayload::new(Platform::MacOs).with("public.utf8-plain-text", &b"hi"[..]);
    assert_eq!(decode_payload(&mac).unwrap(), RichItem::Text("hi".into()));

    let unix = ClipboardPayload::new(Platform::Unix).with("text/plain;charset=utf-8", &b"hi"[..]);
    assert_eq!(decode_payload(&unix).unwrap(), RichItem::Text("hi".into()));
}

#[test]
fn a_payload_whose_only_flavor_is_unknown_decodes_rather_than_failing() {
    // A private format from another application. There is nothing to parse,
    // and handing back the bytes is strictly better than an error: a clipboard
    // bridge can still forward them, and `encode` will republish them verbatim.
    let payload = ClipboardPayload::new(Platform::Unix)
        .with("application/x-acme-widget", &b"\x00\x01\x02"[..]);

    let item = decode_payload(&payload).unwrap();
    assert_eq!(
        item,
        RichItem::Unknown {
            native: "application/x-acme-widget".into(),
            bytes: vec![0, 1, 2],
        }
    );
    assert_eq!(item.kind(), rich_clipboard::ItemKind::Unknown);
}

#[test]
fn an_empty_payload_is_the_one_thing_that_is_an_error() {
    let payload = ClipboardPayload::new(Platform::Unix);
    assert!(matches!(decode_payload(&payload), Err(Error::EmptyPayload)));
}

#[test]
fn a_metadata_flavor_is_not_an_item_on_its_own() {
    let item = ClipboardItem::new("Preferred DropEffect", &2u32.to_le_bytes()[..]);
    assert!(matches!(
        decode(&item, Platform::Windows),
        Err(Error::NotContent { .. })
    ));
}

#[cfg(feature = "rtf")]
#[test]
fn rtf_outranks_plain_text_so_the_styling_survives() {
    let rtf = fixture("rclip-rtf", "minimal.bin");
    let payload = ClipboardPayload::new(Platform::MacOs)
        .with("public.utf8-plain-text", &b"Hello, world!"[..])
        .with("public.rtf", rtf);

    match decode_payload(&payload).unwrap() {
        RichItem::RichText(text) => assert_eq!(text.as_str(), "Hello, world!\n"),
        other => panic!("expected styled text, got {other:?}"),
    }
}

#[cfg(all(feature = "rtf", feature = "html"))]
#[test]
fn rtf_outranks_html_because_it_is_the_one_that_becomes_structure() {
    // `Flavor::read_rank` puts RTF above HTML, and the reason is visible here:
    // there is no HTML tokenizer in the workspace, so the HTML branch can only
    // hand back markup while the RTF branch hands back runs.
    let payload = ClipboardPayload::new(Platform::Windows)
        .with(
            "HTML Format",
            fixture("rclip-cf-html", "zero-padded-offsets.bin"),
        )
        .with("Rich Text Format", fixture("rclip-rtf", "minimal.bin"));

    assert!(matches!(
        decode_payload(&payload).unwrap(),
        RichItem::RichText(_)
    ));
}

#[cfg(not(feature = "rtf"))]
#[test]
fn a_disabled_feature_falls_through_to_the_next_best_flavor() {
    // Not an error: the paste still works, it is just worse. This is the whole
    // reason the read side ranks rather than switches.
    let payload = ClipboardPayload::new(Platform::MacOs)
        .with("public.utf8-plain-text", &b"Hello, world!"[..])
        .with("public.rtf", &b"{\\rtf1\\ansi Hello, world!}"[..]);

    assert_eq!(
        decode_payload(&payload).unwrap(),
        RichItem::Text("Hello, world!".into())
    );
}

#[cfg(not(feature = "rtf"))]
#[test]
fn a_disabled_feature_names_itself_when_nothing_else_is_on_offer() {
    let payload =
        ClipboardPayload::new(Platform::MacOs).with("public.rtf", &b"{\\rtf1\\ansi hi}"[..]);
    assert!(matches!(
        decode_payload(&payload),
        Err(Error::FeatureDisabled { feature: "rtf", .. })
    ));
}

// ---------------------------------------------------------------------------
// Per-format decoding, against the synthetic corpus
// ---------------------------------------------------------------------------

#[cfg(feature = "html")]
#[test]
fn cf_html_gives_back_the_fragment_the_context_and_the_source_url() {
    // With `keep_html_markup`, because `SourceURL` and the context document are
    // things only the markup form carries — a `RichText` has nowhere to put
    // either, which is exactly what the option is for.
    let payload = ClipboardPayload::new(Platform::Windows).with(
        "HTML Format",
        fixture("rclip-cf-html", "zero-padded-offsets.bin"),
    );
    let options = rich_clipboard::Options::new().keep_html_markup(true);

    match rich_clipboard::decode_payload_with(&payload, &options).unwrap() {
        RichItem::Html(html) => {
            assert_eq!(html.markup, "<p>Hello, <b>wörld</b> — café</p>");
            assert_eq!(
                html.source_url.as_deref(),
                Some("https://example.org/article?q=1#frag")
            );
            assert!(html.context.is_some());
            // And the plain text no longer has to come from a sibling flavor:
            // there is a tokenizer now.
            assert_eq!(html.plain.as_deref(), Some("Hello, wörld — café"));
        }
        other => panic!("expected html, got {other:?}"),
    }
}

#[cfg(feature = "html")]
#[test]
fn cf_html_decodes_to_styled_runs_by_default() {
    // The last leg of the hub: an HTML flavor becomes runs, not markup.
    let payload = ClipboardPayload::new(Platform::Windows).with(
        "HTML Format",
        fixture("rclip-cf-html", "zero-padded-offsets.bin"),
    );

    match decode_payload(&payload).unwrap() {
        RichItem::RichText(text) => {
            assert_eq!(text.text, "Hello, wörld — café");
            let bold: String = text
                .spans()
                .filter(|(_, s)| s.bold)
                .map(|(t, _)| t)
                .collect();
            assert_eq!(bold, "wörld");
        }
        other => panic!("expected styled text, got {other:?}"),
    }
}

#[cfg(feature = "html")]
#[test]
fn a_plain_text_sibling_becomes_the_html_fragments_fallback() {
    // A per-item decode cannot see this; only `decode_payload` can, which is
    // why the enrichment lives there.
    let payload = ClipboardPayload::new(Platform::Unix)
        .with("text/html", &b"<b>hi</b>"[..])
        .with("text/plain;charset=utf-8", &b"hi"[..]);
    let options = rich_clipboard::Options::new().keep_html_markup(true);

    match rich_clipboard::decode_payload_with(&payload, &options).unwrap() {
        RichItem::Html(html) => assert_eq!(html.plain.as_deref(), Some("hi")),
        other => panic!("expected html, got {other:?}"),
    }
}

#[cfg(feature = "rtf")]
#[test]
fn rtf_runs_carry_their_font_size_and_colour() {
    let payload = ClipboardPayload::new(Platform::Windows).with(
        "Rich Text Format",
        fixture("rclip-rtf", "font-color-table.bin"),
    );

    let RichItem::RichText(text) = decode_payload(&payload).unwrap() else {
        panic!("expected styled text");
    };
    assert_eq!(text.as_str(), "red blue bold back\n");

    let spans: Vec<_> = text.spans().collect();
    assert_eq!(spans[0].0, "red ");
    assert_eq!(spans[0].1.color, Some(rich_clipboard::Rgb::new(255, 0, 0)));
    assert_eq!(spans[0].1.font_family.as_deref(), Some("Helvetica"));
    // `\fs28` is 28 half-points.
    assert_eq!(spans[0].1.size_pt, Some(14.0));

    assert_eq!(spans[1].0, "blue bold");
    assert!(spans[1].1.bold);
    assert_eq!(spans[1].1.color, Some(rich_clipboard::Rgb::new(0, 0, 255)));
    assert_eq!(spans[1].1.font_family.as_deref(), Some("Courier New"));

    // `\cf0` is the colour table's omitted "auto" entry: inherit, not black.
    assert_eq!(spans[2].0, " back\n");
    assert!(!spans[2].1.bold);
    assert_eq!(spans[2].1.color, None);
}

#[cfg(feature = "file-list")]
#[test]
fn a_windows_file_list_decodes_to_paths() {
    let payload = ClipboardPayload::new(Platform::Windows)
        .with("CF_HDROP", fixture("rclip-dropfiles", "two-paths-wide.bin"));

    let RichItem::Files(list) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(list.len(), 2);
    assert_eq!(
        list.entries[0].as_path(),
        Some("C:\\Users\\alice\\report.pdf")
    );
    // The second path is non-ASCII on purpose: a UTF-16 decode that drops the
    // high byte reads it as `foo.png`.
    assert_eq!(
        list.entries[1].as_path(),
        Some("C:\\Users\\alice\\Bilder\\föö.png")
    );
}

#[cfg(feature = "file-list")]
#[test]
fn a_uri_list_decodes_percent_encoded_paths() {
    let payload = ClipboardPayload::new(Platform::Unix).with(
        "text/uri-list",
        fixture("rclip-uri-list", "two-files-crlf.bin"),
    );

    let RichItem::Files(list) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(list.entries[0].as_path(), Some("/home/me/a.txt"));
    assert_eq!(list.entries[1].as_path(), Some("/home/me/b c.txt"));
}

#[cfg(feature = "file-list")]
#[test]
fn a_gnome_cut_is_read_as_a_move_and_not_as_a_copy() {
    // Without the verb the user's cut silently becomes a copy, which is the
    // whole reason `x-special/gnome-copied-files` exists.
    let payload = ClipboardPayload::new(Platform::Unix)
        .with(
            "text/uri-list",
            fixture("rclip-uri-list", "two-files-crlf.bin"),
        )
        .with(
            "x-special/gnome-copied-files",
            fixture("rclip-uri-list", "gnome-cut.bin"),
        );

    let RichItem::Files(list) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(list.action, rich_clipboard::TransferAction::Move);
    assert_eq!(rich_clipboard::transfer_action(&payload), list.action);
}

#[cfg(feature = "file-list")]
#[test]
fn a_windows_drop_effect_reaches_the_file_list() {
    let payload = ClipboardPayload::new(Platform::Windows)
        .with("CF_HDROP", fixture("rclip-dropfiles", "two-paths-wide.bin"))
        .with("Preferred DropEffect", &2u32.to_le_bytes()[..]);

    let RichItem::Files(list) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(list.action, rich_clipboard::TransferAction::Move);
}

#[cfg(feature = "file-list")]
#[test]
fn a_macos_multi_file_selection_is_reassembled_from_its_items() {
    // A macOS pasteboard models this as N *items*, each offering one
    // `public.file-url`. A per-item decode sees one file — which is exactly
    // what `-[NSPasteboard dataForType:]` gives a transport that does not know
    // about items — and `decode_payload` puts the selection back together.
    let payload = ClipboardPayload::new(Platform::MacOs)
        .with_in(0, "public.file-url", &b"file:///Users/me/a.txt"[..])
        .with_in(1, "public.file-url", &b"file:///Users/me/b%20c.txt"[..])
        .with_in(2, "public.file-url", &b"file:///Users/me/c.txt"[..]);
    assert_eq!(payload.item_count(), 3);

    // What the naive read would have given: item 0 alone, one file.
    let RichItem::Files(one) = decode(&payload.items()[0], Platform::MacOs).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(one.len(), 1);

    let RichItem::Files(list) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(list.len(), 3);
    assert_eq!(list.entries[1].as_path(), Some("/Users/me/b c.txt"));
}

#[cfg(feature = "file-list")]
#[test]
fn items_are_reassembled_in_item_order_and_not_in_arrival_order() {
    // A transport that enumerated the pasteboard by type rather than by item
    // hands the representations over out of order. The user's selection order
    // is the item index, so that is what the reassembly follows.
    let payload = ClipboardPayload::new(Platform::MacOs)
        .with_in(2, "public.file-url", &b"file:///c.txt"[..])
        .with_in(0, "public.file-url", &b"file:///a.txt"[..])
        .with_in(1, "public.file-url", &b"file:///b.txt"[..]);

    let RichItem::Files(list) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    let paths: Vec<_> = list.entries.iter().filter_map(|e| e.as_path()).collect();
    assert_eq!(paths, ["/a.txt", "/b.txt", "/c.txt"]);
}

#[cfg(feature = "file-list")]
#[test]
fn a_transport_that_does_not_track_items_still_gets_every_file() {
    // Item indices are new; a transport that leaves them all at zero is not
    // wrong, just less informative. Every representation still gets collected,
    // because the reassembly is by flavor and the sort by item is only a
    // tie-broken ordering.
    let payload = ClipboardPayload::new(Platform::MacOs)
        .with("public.file-url", &b"file:///a.txt"[..])
        .with("public.file-url", &b"file:///b.txt"[..]);
    assert_eq!(payload.item_count(), 1);

    let RichItem::Files(list) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(list.len(), 2);
}

#[cfg(feature = "file-list")]
#[test]
fn a_real_two_file_finder_copy_comes_back_as_two_files() {
    // `corpus/macos/finder/`: two `NSPasteboardItem`s captured off the general
    // pasteboard after selecting two files in Finder and pressing Cmd-C. Note
    // what Finder actually puts there — the opaque `file:///.file/id=` form,
    // which only the file system can resolve. It stays a path-shaped string
    // and nothing here resolves it.
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/macos/finder/");
    let payload = ClipboardPayload::new(Platform::MacOs)
        .with_in(
            0,
            "public.file-url",
            std::fs::read(format!("{root}item-00.public.file-url.bin")).expect("capture"),
        )
        .with_in(
            1,
            "public.file-url",
            std::fs::read(format!("{root}item-01.public.file-url.bin")).expect("capture"),
        );

    let RichItem::Files(list) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(list.len(), 2);
    assert_eq!(
        list.entries[0].as_path(),
        Some("/.file/id=6571367.381000889")
    );
    assert_eq!(
        list.entries[1].as_path(),
        Some("/.file/id=6571367.381000890")
    );
}

#[cfg(feature = "file-list")]
#[test]
fn a_macos_file_list_survives_a_publish_and_a_read_back() {
    use rich_clipboard::{encode, FileList};

    // The loop a transport runs, with the grouping in the middle: `encode`
    // emits one item per file and `decode_payload` reassembles the same list.
    let before = FileList::of_paths(["/Users/me/a b.txt", "/Users/me/c.txt", "/Users/me/d.txt"]);
    let payload = encode(&RichItem::Files(before.clone()), Platform::MacOs).unwrap();

    let RichItem::Files(after) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(after.entries, before.entries);
}

#[cfg(feature = "dib")]
#[test]
fn a_top_down_dib_decodes_to_the_same_pixels_as_its_bottom_up_twin() {
    // `biHeight` negative means top-down. Getting it backwards flips the image
    // and nothing errors.
    let top = ClipboardPayload::new(Platform::Windows)
        .with("CF_DIB", fixture("rclip-dib", "24bpp-top-down-2x2.bin"));
    let bottom = ClipboardPayload::new(Platform::Windows)
        .with("CF_DIB", fixture("rclip-dib", "24bpp-bottom-up-2x2.bin"));

    let pixels = |p: &ClipboardPayload| match decode_payload(p).unwrap() {
        RichItem::Image(rich_clipboard::Image::Rgba(img)) => {
            assert_eq!((img.width, img.height), (2, 2));
            img.pixels
        }
        other => panic!("expected pixels, got {other:?}"),
    };
    assert_eq!(pixels(&top), pixels(&bottom));
}

#[test]
fn png_is_handed_over_encoded_because_this_workspace_does_not_decode_it() {
    let payload = ClipboardPayload::new(Platform::Unix).with("image/png", &b"\x89PNG\r\n"[..]);
    match decode_payload(&payload).unwrap() {
        RichItem::Image(rich_clipboard::Image::Encoded { format, bytes }) => {
            assert_eq!(format, rich_clipboard::ImageFormat::Png);
            assert_eq!(bytes, b"\x89PNG\r\n");
        }
        other => panic!("expected encoded image, got {other:?}"),
    }
}

#[cfg(feature = "id-list")]
#[test]
fn a_shell_id_list_yields_display_labels_and_never_a_path_to_open() {
    let payload = ClipboardPayload::new(Platform::Windows).with(
        "Shell IDList Array",
        fixture("rclip-idlist", "cida-two-children.bin"),
    );

    let RichItem::ShellItems(items) = decode_payload(&payload).unwrap() else {
        panic!("expected shell items");
    };
    assert_eq!(items.display_paths.len(), 2);
    assert!(items.parent.is_some());
}

#[cfg(feature = "file-desc")]
#[test]
fn a_clear_size_flag_reads_as_unstated_and_not_as_zero() {
    let payload = ClipboardPayload::new(Platform::Windows).with(
        "FileGroupDescriptorW",
        fixture("rclip-file-desc", "two-descriptors.bin"),
    );

    let RichItem::PromisedFiles(files) = decode_payload(&payload).unwrap() else {
        panic!("expected promised files");
    };
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].size, Some((4 << 30) + 32));
    // The second descriptor's size field holds 0xDEADBEEF with FD_FILESIZE
    // clear. A reader that ignores dwFlags reports a 16-exabyte directory.
    assert_eq!(files[1].size, None);
    assert!(files[1].is_directory);
}

#[cfg(feature = "shell-link")]
#[test]
fn a_shell_link_is_parsed_and_nothing_is_resolved() {
    // Not via `decode`: `Flavor::ShellLink` has no identifier on any platform,
    // because Windows has no registered clipboard format for a shell link. A
    // `.lnk` arrives as a *file*, so the conversion is a direct one.
    let link =
        rich_clipboard::Shortcut::from_lnk(&fixture("rclip-shell-link", "with-string-data.bin"))
            .unwrap();

    assert_eq!(link.name.as_deref(), Some("My Notes"));
    assert_eq!(link.arguments.as_deref(), Some("--flag \"a b\""));
    assert_eq!(
        link.icon_location.as_deref(),
        Some("%SystemRoot%\\system32\\shell32.dll")
    );

    // A name Windows would resolve, that this crate does not: the string is
    // handed back unexpanded and no file is touched.
    assert_eq!(link.working_dir.as_deref(), Some("C:\\Users\\me"));
}

#[cfg(feature = "html")]
#[test]
fn malformed_bytes_report_the_codec_error_rather_than_being_swallowed() {
    let payload = ClipboardPayload::new(Platform::Windows).with(
        "HTML Format",
        fixture("rclip-cf-html", "bare-html-no-header.bin"),
    );

    match decode_payload(&payload) {
        Err(Error::Codec { native, source }) => {
            assert_eq!(native, "HTML Format");
            assert_eq!(source.kind, rclip_core::ErrorKind::BadMagic);
        }
        other => panic!("expected a codec error, got {other:?}"),
    }
}

#[cfg(feature = "html")]
#[test]
fn when_every_flavor_fails_a_codec_error_is_what_is_reported() {
    // Malformed bytes are the more urgent thing to hear about, so a codec
    // failure displaces a `FeatureDisabled` recorded from a higher-ranked
    // flavor.
    let payload = ClipboardPayload::new(Platform::Windows)
        .with("Shell IDList Array", &b"\x00"[..])
        .with("HTML Format", &b"not cf_html at all"[..]);

    assert!(matches!(decode_payload(&payload), Err(Error::Codec { .. })));
}

// ---------------------------------------------------------------------------
// Size limits. These are the facade's half of PLAN.md §4b; the transport owns
// the other half, before it reads.
// ---------------------------------------------------------------------------

#[test]
fn an_oversize_flavor_is_skipped_in_favour_of_a_smaller_one() {
    use rclip_core::Limits;

    // What a real paste of a big screenshot plus its caption looks like: the
    // image is huge, the text is not. Refusing the image must leave the text.
    let big = vec![0u8; 4096];
    let payload = ClipboardPayload::new(Platform::MacOs)
        .with("public.tiff", big)
        .with("public.utf8-plain-text", b"caption".to_vec());

    let opts = rich_clipboard::Options::new().limits(Limits {
        max_flavor_bytes: 1024,
        ..Limits::default()
    });

    let item = rich_clipboard::decode_payload_with(&payload, &opts)
        .expect("skipping the image must still yield the text");
    assert_eq!(item.plain_text(), Some("caption"));
}

#[test]
fn the_policy_sees_the_flavor_and_its_exact_size() {
    use rclip_core::{Flavor, Limits, Oversize, SizeHint};

    let payload = ClipboardPayload::new(Platform::MacOs)
        .with("public.tiff", vec![0u8; 4096])
        .with("public.utf8-plain-text", b"caption".to_vec());
    let opts = rich_clipboard::Options::new().limits(Limits {
        max_flavor_bytes: 1024,
        ..Limits::default()
    });

    let mut seen = Vec::new();
    let item = rich_clipboard::decode_payload_policy(
        &payload,
        &opts,
        &mut |f: Flavor<'_>, h: SizeHint, _: &Limits| {
            seen.push((format!("{f:?}"), h));
            Oversize::Skip
        },
    )
    .unwrap();

    assert_eq!(item.plain_text(), Some("caption"));
    assert_eq!(seen.len(), 1, "only the oversize flavor is reported");
    assert_eq!(seen[0].0, "Tiff");
    assert_eq!(
        seen[0].1,
        SizeHint::Exact(4096),
        "the bytes are in hand here, so the size is exact rather than a bound"
    );
}

#[test]
fn accept_overrides_the_limit_for_that_flavor_only() {
    use rclip_core::{Flavor, Limits, Oversize, SizeHint};

    let payload =
        ClipboardPayload::new(Platform::MacOs).with("public.utf8-plain-text", vec![b'x'; 4096]);
    let opts = rich_clipboard::Options::new().limits(Limits {
        max_flavor_bytes: 16,
        ..Limits::default()
    });

    let item = rich_clipboard::decode_payload_policy(
        &payload,
        &opts,
        &mut |_: Flavor<'_>, _: SizeHint, _: &Limits| Oversize::Accept,
    )
    .expect("Accept means decode it anyway");
    assert_eq!(item.plain_text().map(|s| s.len()), Some(4096));
}

#[test]
fn abort_fails_the_whole_paste() {
    use rclip_core::{Flavor, Limits, Oversize, SizeHint};

    let payload = ClipboardPayload::new(Platform::MacOs)
        .with("public.tiff", vec![0u8; 4096])
        .with("public.utf8-plain-text", b"caption".to_vec());
    let opts = rich_clipboard::Options::new().limits(Limits {
        max_flavor_bytes: 1024,
        ..Limits::default()
    });

    let err = rich_clipboard::decode_payload_policy(
        &payload,
        &opts,
        &mut |_: Flavor<'_>, _: SizeHint, _: &Limits| Oversize::Abort,
    )
    .expect_err("Abort must not fall through to the text");
    assert!(
        matches!(err, rich_clipboard::Error::Oversize { .. }),
        "{err:?}"
    );
}

#[test]
fn refusing_everything_reports_the_largest_offender() {
    use rclip_core::Limits;

    let payload = ClipboardPayload::new(Platform::MacOs)
        .with("public.utf8-plain-text", vec![b'x'; 2048])
        .with("public.tiff", vec![0u8; 9000]);
    let opts = rich_clipboard::Options::new().limits(Limits {
        max_flavor_bytes: 16,
        ..Limits::default()
    });

    match rich_clipboard::decode_payload_with(&payload, &opts) {
        Err(rich_clipboard::Error::Oversize {
            flavor,
            bytes,
            limit,
        }) => {
            // The largest, not the last looked at — that is the number someone
            // needs when they ask why the paste did nothing.
            assert_eq!(flavor, "Tiff");
            assert_eq!(bytes, 9000);
            assert_eq!(limit, 16);
        }
        other => panic!("expected Oversize, got {other:?}"),
    }
}

#[test]
fn the_default_limits_do_not_interfere_with_a_realistic_paste() {
    // A regression guard on the defaults themselves: anything a person
    // actually copies must sail through without a policy being involved.
    let payload =
        ClipboardPayload::new(Platform::MacOs).with("public.utf8-plain-text", vec![b'x'; 1 << 20]);
    let item = rich_clipboard::decode_payload(&payload).expect("1 MiB of text is ordinary");
    assert_eq!(item.plain_text().map(|s| s.len()), Some(1 << 20));
}

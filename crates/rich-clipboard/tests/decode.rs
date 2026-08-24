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
fn a_macos_multi_file_selection_is_reassembled_from_its_repeated_items() {
    // A macOS pasteboard models this as N items sharing one type. A per-item
    // decode sees one file; `decode_payload` collects the rest.
    let payload = ClipboardPayload::new(Platform::MacOs)
        .with("public.file-url", &b"file:///Users/me/a.txt"[..])
        .with("public.file-url", &b"file:///Users/me/b%20c.txt"[..]);

    let RichItem::Files(list) = decode_payload(&payload).unwrap() else {
        panic!("expected files");
    };
    assert_eq!(list.len(), 2);
    assert_eq!(list.entries[1].as_path(), Some("/Users/me/b c.txt"));
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

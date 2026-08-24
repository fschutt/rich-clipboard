//! The fan-out: one item in, several flavors out, per platform.

use rclip_core::{ClipboardPayload, Platform};
use rich_clipboard::{encode, Error, RichItem};

fn natives(payload: &ClipboardPayload) -> Vec<&str> {
    payload.items().iter().map(|i| i.native.as_str()).collect()
}

fn bytes_for<'a>(payload: &'a ClipboardPayload, native: &str) -> &'a [u8] {
    payload
        .items()
        .iter()
        .find(|i| i.native == native)
        .map(|i| i.bytes.as_slice())
        .unwrap_or_else(|| panic!("{native} was not published"))
}

#[cfg(any(feature = "rtf", feature = "html"))]
fn styled() -> rich_clipboard::RichText {
    use rich_clipboard::{Rgb, Style};

    let mut text = rich_clipboard::RichText::default();
    text.push("plain ", Style::default());
    text.push(
        "bold red",
        Style {
            bold: true,
            color: Some(Rgb::new(255, 0, 0)),
            font_family: Some("Courier New".into()),
            size_pt: Some(14.0),
            ..Style::default()
        },
    );
    text
}

// ---------------------------------------------------------------------------
// Plain text — the one kind that needs no format feature
// ---------------------------------------------------------------------------

#[test]
fn plain_text_publishes_one_flavor_per_platform_in_that_platforms_encoding() {
    let item = RichItem::Text("hi".into());

    let win = encode(&item, Platform::Windows).unwrap();
    assert_eq!(natives(&win), ["CF_UNICODETEXT"]);
    // UTF-16LE and NUL-terminated. A consumer calling `lstrlenW` on a
    // payload without the terminator reads past the allocation.
    assert_eq!(bytes_for(&win, "CF_UNICODETEXT"), b"h\0i\0\0\0");

    let mac = encode(&item, Platform::MacOs).unwrap();
    assert_eq!(natives(&mac), ["public.utf8-plain-text"]);
    assert_eq!(bytes_for(&mac, "public.utf8-plain-text"), b"hi");

    let unix = encode(&item, Platform::Unix).unwrap();
    assert_eq!(natives(&unix), ["text/plain;charset=utf-8"]);
}

#[test]
fn an_unknown_flavor_is_republished_under_the_name_it_arrived_with() {
    // What makes a clipboard bridge possible: bytes this build cannot read
    // still travel intact.
    let item = RichItem::Unknown {
        native: "application/x-acme-widget".into(),
        bytes: vec![1, 2, 3],
    };
    let payload = encode(&item, Platform::Unix).unwrap();
    assert_eq!(natives(&payload), ["application/x-acme-widget"]);
    assert_eq!(bytes_for(&payload, "application/x-acme-widget"), [1, 2, 3]);
}

#[test]
fn a_kind_with_no_representation_says_so_instead_of_publishing_nothing() {
    let shortcut = RichItem::Shortcut(rich_clipboard::Shortcut::default());
    assert!(matches!(
        encode(&shortcut, Platform::Windows),
        Err(Error::NotPublishable { .. })
    ));

    let promised = RichItem::PromisedFiles(vec![]);
    assert!(matches!(
        encode(&promised, Platform::Unix),
        Err(Error::NotPublishable { .. })
    ));
}

// ---------------------------------------------------------------------------
// Styled text: the three-flavor fan-out
// ---------------------------------------------------------------------------

#[cfg(feature = "rich-text")]
#[test]
fn styled_text_goes_out_as_three_flavors_on_windows() {
    let payload = encode(&RichItem::RichText(styled()), Platform::Windows).unwrap();
    assert_eq!(
        natives(&payload),
        ["Rich Text Format", "HTML Format", "CF_UNICODETEXT"]
    );

    // Each one is really the thing it claims to be, not a stub.
    assert!(bytes_for(&payload, "Rich Text Format").starts_with(br"{\rtf1"));
    let html = bytes_for(&payload, "HTML Format");
    assert!(html.starts_with(b"Version:"));
    assert_eq!(
        rclip_cf_html::parse(html).unwrap().fragment,
        rich_clipboard::RichText::to_html_fragment(&styled())
    );
    assert_eq!(bytes_for(&payload, "CF_UNICODETEXT"), {
        let mut v: Vec<u8> = "plain bold red"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        v.extend_from_slice(&[0, 0]);
        v
    });
}

#[cfg(feature = "rtf")]
#[test]
fn styled_text_on_macos_leads_with_rtf_and_offers_html_behind_it() {
    // The order is a stated preference and not a mechanism: what decides an
    // AppKit paste is `-[NSTextView readablePasteboardTypes]`, which is ordered
    // RTFD, RTF, HTML — the *reader's* list, not ours. Publishing HTML as well
    // therefore cannot cost an AppKit consumer anything, and it is what a
    // WebKit or Chromium consumer needs to get the styling first-hand rather
    // than through Cocoa's lossy RTF-to-HTML conversion.
    let payload = encode(&RichItem::RichText(styled()), Platform::MacOs).unwrap();
    #[cfg(feature = "html")]
    assert_eq!(
        natives(&payload),
        ["public.rtf", "public.html", "public.utf8-plain-text"]
    );
    #[cfg(not(feature = "html"))]
    assert_eq!(natives(&payload), ["public.rtf", "public.utf8-plain-text"]);

    assert!(bytes_for(&payload, "public.rtf").starts_with(br"{\rtf1"));
}

#[cfg(all(feature = "rtf", feature = "html"))]
#[test]
fn macos_html_is_bare_markup_and_not_a_cf_html_blob() {
    // `public.html` is the markup with nothing around it. A CF_HTML header
    // here would reach WebKit as literal `Version:1.0` text.
    let payload = encode(&RichItem::RichText(styled()), Platform::MacOs).unwrap();
    let html = bytes_for(&payload, "public.html");
    assert!(!html.starts_with(b"Version:"));
    assert!(core::str::from_utf8(html)
        .unwrap()
        .contains("font-weight:700"));
}

#[cfg(feature = "html")]
#[test]
fn styled_text_on_unix_is_bare_markup_with_no_cf_html_header() {
    let payload = encode(&RichItem::RichText(styled()), Platform::Unix).unwrap();
    // HTML first here and RTF second, the one place this table inverts the
    // Windows and macOS order: Qt's rich text *is* an HTML subset and GTK's
    // rich-text target is its own internal serialization, so HTML is what the
    // toolkits take and RTF is for the applications underneath them.
    #[cfg(feature = "rtf")]
    assert_eq!(
        natives(&payload),
        ["text/html", "text/rtf", "text/plain;charset=utf-8"]
    );
    #[cfg(not(feature = "rtf"))]
    assert_eq!(natives(&payload), ["text/html", "text/plain;charset=utf-8"]);

    let html = bytes_for(&payload, "text/html");
    assert!(
        !html.starts_with(b"Version:"),
        "the CF_HTML header leaked onto Unix"
    );
    assert!(core::str::from_utf8(html)
        .unwrap()
        .contains("font-weight:700"));
}

#[cfg(feature = "rtf")]
#[test]
fn the_unix_rtf_offer_is_real_rtf_and_is_never_called_text_richtext() {
    // `text/richtext` is RFC 1896 enriched text. LibreOffice advertises RTF
    // under it on X11 anyway, which is why the read side accepts that spelling
    // — but emitting under it would be mislabelling, so the write side does
    // not.
    let payload = encode(&RichItem::RichText(styled()), Platform::Unix).unwrap();
    assert!(bytes_for(&payload, "text/rtf").starts_with(br"{\rtf1"));
    assert!(!natives(&payload).contains(&"text/richtext"));
}

#[cfg(all(feature = "html", not(feature = "rtf")))]
#[test]
fn a_missing_codec_costs_a_flavor_and_not_the_publish() {
    // Word gets HTML instead of RTF. Worse, but not broken — which is why a
    // flavor this build cannot produce is skipped rather than fatal.
    let payload = encode(&RichItem::RichText(styled()), Platform::Windows).unwrap();
    assert_eq!(natives(&payload), ["HTML Format", "CF_UNICODETEXT"]);
}

#[cfg(feature = "html")]
#[test]
fn a_raw_html_fragment_publishes_its_plain_fallback_only_when_it_has_one() {
    use rich_clipboard::HtmlFragment;

    let bare = RichItem::Html(HtmlFragment {
        markup: "<b>hi</b>".into(),
        ..HtmlFragment::default()
    });
    // No tokenizer, so no honest plain text to derive. Tag-stripping would
    // produce something that looks like the text and is not.
    assert_eq!(
        natives(&encode(&bare, Platform::Unix).unwrap()),
        ["text/html"]
    );

    let with_plain = RichItem::Html(HtmlFragment {
        markup: "<b>hi</b>".into(),
        plain: Some("hi".into()),
        ..HtmlFragment::default()
    });
    assert_eq!(
        natives(&encode(&with_plain, Platform::Unix).unwrap()),
        ["text/html", "text/plain;charset=utf-8"]
    );
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

#[cfg(feature = "file-list")]
#[test]
fn a_linux_cut_publishes_every_convention_and_agrees_with_itself() {
    use rich_clipboard::{FileList, TransferAction};

    let mut list = FileList::of_paths(["/home/me/a b.txt", "/home/me/c.txt"]);
    list.action = TransferAction::Move;
    let payload = encode(&RichItem::Files(list), Platform::Unix).unwrap();

    assert_eq!(
        natives(&payload),
        [
            "text/uri-list",
            "x-special/gnome-copied-files",
            "x-special/mate-copied-files",
            "application/x-kde-cutselection",
        ]
    );

    // `text/uri-list` is CRLF-terminated after every URI, including the last.
    assert_eq!(
        bytes_for(&payload, "text/uri-list"),
        b"file:///home/me/a%20b.txt\r\nfile:///home/me/c.txt\r\n"
    );
    // The GNOME payload is LF, verb-first, and has *no* trailing newline:
    // Nautilus 44+ rejects the whole payload if any line after the verb is
    // empty, so a trailing newline makes the paste do nothing at all.
    assert_eq!(
        bytes_for(&payload, "x-special/gnome-copied-files"),
        b"cut\nfile:///home/me/a%20b.txt\nfile:///home/me/c.txt"
    );
    assert_eq!(bytes_for(&payload, "application/x-kde-cutselection"), b"1");
}

#[cfg(feature = "file-list")]
#[test]
fn a_windows_file_copy_carries_a_drop_effect_word() {
    use rich_clipboard::FileList;

    let payload = encode(
        &RichItem::Files(FileList::of_paths(["C:\\a.txt"])),
        Platform::Windows,
    )
    .unwrap();
    assert_eq!(natives(&payload), ["CF_HDROP", "Preferred DropEffect"]);
    assert_eq!(
        bytes_for(&payload, "Preferred DropEffect"),
        1u32.to_le_bytes()
    );
    assert_eq!(bytes_for(&payload, "CF_HDROP")[16], 1, "fWide must be set");
}

#[cfg(feature = "file-list")]
#[test]
fn a_macos_file_list_becomes_one_pasteboard_item_per_file() {
    use rich_clipboard::FileList;

    let payload = encode(
        &RichItem::Files(FileList::of_paths(["/a.txt", "/b.txt", "/c.txt"])),
        Platform::MacOs,
    )
    .unwrap();
    assert_eq!(
        natives(&payload),
        ["public.file-url", "public.file-url", "public.file-url"]
    );
    // The part that is not cosmetic: three *items*, not one item offering the
    // type three times. `-[NSPasteboard dataForType:]` reaches only the first
    // item that offers a type, so the grouping is the difference between the
    // receiver seeing three files and seeing one.
    assert_eq!(
        payload.items().iter().map(|i| i.item).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(payload.item_count(), 3);
    assert_eq!(payload.group(1).count(), 1);
}

#[cfg(feature = "file-list")]
#[test]
fn a_file_uri_is_encoded_the_way_g_filename_to_uri_encodes_one() {
    use rich_clipboard::FileList;

    // The set is `EncodeSet::Path` — RFC 3986 `pchar` plus `/`. Sub-delims and
    // `@` are legal in a URI path and stay literal; escaping them would
    // produce a URI that no GTK application ever produces, and RFC 3986
    // §6.2.2.2 says a percent-encoded reserved character is *not* equal to its
    // literal form, so a receiver comparing URIs textually stops matching.
    let list = FileList::of_paths(["/tmp/it's (1)&2, me@host.txt", "/tmp/a b#c?d%e.txt"]);
    let payload = encode(&RichItem::Files(list), Platform::Unix).unwrap();
    assert_eq!(
        bytes_for(&payload, "text/uri-list"),
        concat!(
            "file:///tmp/it's%20(1)&2,%20me@host.txt\r\n",
            "file:///tmp/a%20b%23c%3Fd%25e.txt\r\n",
        )
        .as_bytes()
    );
}

#[cfg(feature = "file-list")]
#[test]
fn the_one_byte_where_this_and_g_filename_to_uri_disagree_is_the_semicolon() {
    use rich_clipboard::FileList;

    // Pinned because it was measured, not assumed: run against real GLib
    // 2.88, `emit::file_uri` agrees with `g_filename_to_uri` on every
    // printable ASCII byte and on multi-byte UTF-8 — except `;`, which GLib
    // escapes as `%3B`. `g_filename_to_uri` does not use
    // `G_URI_RESERVED_CHARS_ALLOWED_IN_PATH`; it escapes against an unsafe
    // list whose allowed reserved set is `:@&=+$,` plus `/`.
    //
    // Both forms decode to the same path, so this is an interop nit and not a
    // correctness bug — but it is the kind that goes unnoticed until a
    // receiver compares URI strings, so it is a test rather than a comment.
    let payload = encode(
        &RichItem::Files(FileList::of_paths(["/t/a;b"])),
        Platform::Unix,
    )
    .unwrap();
    assert_eq!(bytes_for(&payload, "text/uri-list"), b"file:///t/a;b\r\n");
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

#[cfg(feature = "dib")]
#[test]
fn pixels_publish_as_both_dib_shapes_on_windows() {
    use rich_clipboard::{Image, RgbaImage};

    let img = RichItem::Image(Image::Rgba(RgbaImage {
        width: 1,
        height: 1,
        pixels: vec![0x11, 0x22, 0x33, 0xFF],
    }));
    let payload = encode(&img, Platform::Windows).unwrap();
    // `CF_DIB` as well as `CF_DIBV5`, because the Win32 applications that read
    // one and not the other are exactly the old ones, and they are the reason
    // the plan leads with DIB in the first place.
    #[cfg(not(feature = "image"))]
    assert_eq!(natives(&payload), ["CF_DIBV5", "CF_DIB"]);
    #[cfg(feature = "image")]
    assert_eq!(natives(&payload), ["CF_DIBV5", "PNG", "CF_DIB"]);

    // The header size field is what tells the two apart, and a consumer
    // switches on it: 124 is `BITMAPV5HEADER`, 40 is `BITMAPINFOHEADER`.
    let header_size =
        |native| u32::from_le_bytes(bytes_for(&payload, native)[..4].try_into().unwrap());
    assert_eq!(header_size("CF_DIBV5"), 124);
    assert_eq!(header_size("CF_DIB"), 40);
    let dib = bytes_for(&payload, "CF_DIB");
    assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 24);
    // 24-bpp DIB pixels are blue, green, red — and bottom-up, which for one
    // row is the same row.
    assert_eq!(&dib[40..43], &[0x33, 0x22, 0x11]);
}

#[cfg(feature = "dib")]
#[test]
fn the_cf_dib_companion_composites_transparency_over_white() {
    use rich_clipboard::{Image, RgbaImage};

    // `BITMAPINFOHEADER` has no alpha channel, so the choice is what happens
    // to it. Compositing over white rather than discarding is why a screenshot
    // with transparent corners pastes into Paint with white corners and not
    // with black ones — in a straight-alpha buffer the colour under a fully
    // transparent pixel is usually zero.
    let img = RichItem::Image(Image::Rgba(RgbaImage {
        width: 2,
        height: 1,
        pixels: vec![0, 0, 0, 0, 0, 0, 0, 128],
    }));
    let payload = encode(&img, Platform::Windows).unwrap();
    let dib = bytes_for(&payload, "CF_DIB");
    // Fully transparent black becomes white, half-transparent black becomes
    // mid grey. `(0 * 128 + 255 * 127 + 127) / 255 == 127`.
    assert_eq!(&dib[40..46], &[0xFF, 0xFF, 0xFF, 127, 127, 127]);
}

#[cfg(all(feature = "dib", not(feature = "image")))]
#[test]
fn pixels_cannot_be_published_on_macos_without_the_image_feature() {
    use rich_clipboard::{Image, RgbaImage};

    // The macOS plan is PNG and TIFF, and `plan/PLAN.md` §4.4 keeps both
    // encoders out of this *workspace*. Without the `image` feature there is
    // nothing to call, and a consumer should encode first and hand over an
    // `Image::Encoded`.
    let img = RichItem::Image(Image::Rgba(RgbaImage {
        width: 1,
        height: 1,
        pixels: vec![0, 0, 0, 255],
    }));
    assert!(matches!(
        encode(&img, Platform::MacOs),
        Err(Error::NothingEncodable { .. })
    ));
}

#[cfg(feature = "image")]
#[test]
fn pixels_publish_as_png_and_tiff_on_macos_with_the_image_feature() {
    use rich_clipboard::{Image, RgbaImage};

    let img = RichItem::Image(Image::Rgba(RgbaImage {
        width: 2,
        height: 2,
        pixels: vec![
            0xFF, 0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, //
            0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x80,
        ],
    }));
    let payload = encode(&img, Platform::MacOs).unwrap();
    assert_eq!(natives(&payload), ["public.png", "public.tiff"]);

    // Read the IHDR rather than round-tripping through the same library that
    // wrote it: an encoder and its own decoder agreeing proves less than the
    // header saying what a foreign reader will act on.
    let png = bytes_for(&payload, "public.png");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(&png[12..16], b"IHDR");
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 2);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 2);
    assert_eq!(png[24], 8, "bit depth");
    // Colour type 6 is truecolour *with alpha*, which is the whole reason
    // `Image::Rgba` is worth publishing on macOS rather than flattening it.
    assert_eq!(png[25], 6, "colour type");

    // TIFF is byte-order-tagged: `II` little-endian or `MM` big-endian, then 42.
    let tiff = bytes_for(&payload, "public.tiff");
    assert!(
        tiff.starts_with(b"II\x2A\x00") || tiff.starts_with(b"MM\x00\x2A"),
        "{:?}",
        &tiff[..4]
    );
}

#[cfg(feature = "image")]
#[test]
fn an_image_whose_buffer_does_not_match_its_dimensions_costs_the_flavor_and_not_the_publish() {
    use rich_clipboard::{Image, RgbaImage};

    // Bad caller data, treated the way every other flavor treats it: drop this
    // one, keep the rest of the fan-out.
    let img = RichItem::Image(Image::Rgba(RgbaImage {
        width: 100,
        height: 100,
        pixels: vec![0; 4],
    }));
    assert!(matches!(
        encode(&img, Platform::MacOs),
        Err(Error::NothingEncodable { .. })
    ));
}

#[test]
fn an_encoded_png_publishes_wherever_the_plan_names_png() {
    use rich_clipboard::{Image, ImageFormat};

    let png = RichItem::Image(Image::Encoded {
        format: ImageFormat::Png,
        bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
    });
    assert_eq!(
        natives(&encode(&png, Platform::Unix).unwrap()),
        ["image/png"]
    );
    assert_eq!(
        natives(&encode(&png, Platform::MacOs).unwrap()),
        ["public.png"]
    );
    // On Windows PNG is second in the plan, behind CF_DIBV5 — which this item
    // cannot fill, because decoding the PNG is out of scope here.
    assert_eq!(natives(&encode(&png, Platform::Windows).unwrap()), ["PNG"]);
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

#[test]
fn a_link_carries_its_title_only_where_the_platform_has_somewhere_to_put_it() {
    let link =
        RichItem::Link(rich_clipboard::Link::to_url("https://example.com/").titled("Example"));

    // macOS has `public.url-name`.
    assert_eq!(
        natives(&encode(&link, Platform::MacOs).unwrap()),
        ["public.url", "public.url-name", "public.utf8-plain-text"]
    );
    // Windows does not: `CFSTR_INETURL` is the URL and nothing else.
    assert_eq!(
        natives(&encode(&link, Platform::Windows).unwrap()),
        ["UniformResourceLocatorW", "CF_UNICODETEXT"]
    );
    // On X11 and Wayland a URL and a file list are the same MIME type.
    assert_eq!(
        natives(&encode(&link, Platform::Unix).unwrap()),
        ["text/uri-list", "text/plain;charset=utf-8"]
    );
}

// ---------------------------------------------------------------------------
// Promised files
// ---------------------------------------------------------------------------

#[cfg(feature = "file-desc")]
#[test]
fn a_promised_file_publishes_a_descriptor_with_its_size_flag_set() {
    use rich_clipboard::PromisedFile;

    let item = RichItem::PromisedFiles(vec![PromisedFile {
        name: "report.pdf".into(),
        size: Some(4096),
        ..PromisedFile::default()
    }]);
    let payload = encode(&item, Platform::Windows).unwrap();
    assert_eq!(natives(&payload), ["FileGroupDescriptorW"]);

    let bytes = bytes_for(&payload, "FileGroupDescriptorW");
    let group = rclip_file_desc::FileGroupDescriptor::parse(bytes).unwrap();
    let d = group.get(0).unwrap();
    assert_eq!(d.file_size(), Some(4096));
    assert_eq!(d.file_name_lossy(), "report.pdf");
}

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
fn styled_text_on_macos_is_rtf_plus_plain_and_not_html() {
    let payload = encode(&RichItem::RichText(styled()), Platform::MacOs).unwrap();
    assert_eq!(natives(&payload), ["public.rtf", "public.utf8-plain-text"]);
}

#[cfg(feature = "html")]
#[test]
fn styled_text_on_unix_is_bare_markup_with_no_cf_html_header() {
    let payload = encode(&RichItem::RichText(styled()), Platform::Unix).unwrap();
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
fn a_macos_file_list_becomes_one_item_per_file() {
    use rich_clipboard::FileList;

    let payload = encode(
        &RichItem::Files(FileList::of_paths(["/a.txt", "/b.txt"])),
        Platform::MacOs,
    )
    .unwrap();
    assert_eq!(natives(&payload), ["public.file-url", "public.file-url"]);
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

#[cfg(feature = "dib")]
#[test]
fn pixels_publish_as_cf_dibv5_on_windows() {
    use rich_clipboard::{Image, RgbaImage};

    let img = RichItem::Image(Image::Rgba(RgbaImage {
        width: 1,
        height: 1,
        pixels: vec![0x11, 0x22, 0x33, 0xFF],
    }));
    let payload = encode(&img, Platform::Windows).unwrap();
    // Not `CF_DIB`: `rclip-dib` writes only the V5 shape, because a producer
    // has no reason to emit a format that cannot carry alpha. The plan
    // advertises the `CF_DIB` line and this build never fills it.
    assert_eq!(natives(&payload), ["CF_DIBV5"]);
    assert_eq!(
        u32::from_le_bytes(bytes_for(&payload, "CF_DIBV5")[..4].try_into().unwrap()),
        124
    );
}

#[cfg(feature = "dib")]
#[test]
fn pixels_cannot_be_published_on_macos_at_all() {
    use rich_clipboard::{Image, RgbaImage};

    // A real gap, asserted rather than hidden: the macOS plan is PNG and TIFF,
    // and `plan/PLAN.md` §4.4 keeps both encoders out of this workspace. A
    // consumer with `image` should encode first and hand over
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

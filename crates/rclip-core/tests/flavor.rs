use rclip_core::{
    flavor::{cf, cfstr, WindowsFormat},
    Flavor,
};

#[test]
fn mime_parameters_do_not_defeat_recognition() {
    assert_eq!(
        Flavor::from_mime("text/plain;charset=utf-8"),
        Flavor::PlainText
    );
    assert_eq!(Flavor::from_mime("text/plain"), Flavor::PlainText);
    assert_eq!(Flavor::from_mime("UTF8_STRING"), Flavor::PlainText);
    assert_eq!(Flavor::from_mime("text/html;charset=UTF-8"), Flavor::Html);
}

#[test]
fn unknown_flavors_survive_as_their_native_name() {
    let f = Flavor::from_mime("application/x-kde-cutselection");
    assert_eq!(f, Flavor::Other("application/x-kde-cutselection"));
    let u = Flavor::from_uti("dyn.ah62d4rv4ge8");
    assert_eq!(u, Flavor::Other("dyn.ah62d4rv4ge8"));
}

#[test]
fn windows_round_trips_through_the_registry() {
    for flavor in [
        Flavor::PlainText,
        Flavor::Html,
        Flavor::Rtf,
        Flavor::FileList,
        Flavor::DibV5,
    ] {
        let win = flavor.windows().expect("must map to a Windows format");
        assert_eq!(
            Flavor::from_windows(win),
            flavor,
            "{flavor:?} did not round-trip"
        );
    }
}

#[test]
fn the_registry_agrees_with_the_platform_constants() {
    assert_eq!(
        Flavor::PlainText.windows(),
        Some(WindowsFormat::Predefined(cf::UNICODETEXT))
    );
    assert_eq!(
        Flavor::FileList.windows(),
        Some(WindowsFormat::Predefined(cf::HDROP))
    );
    assert_eq!(
        Flavor::Html.windows(),
        Some(WindowsFormat::Registered(cfstr::HTML))
    );
    assert_eq!(Flavor::Html.uti(), Some("public.html"));
    assert_eq!(Flavor::FileList.mime(), Some("text/uri-list"));
}

#[test]
fn rich_text_outranks_plain_text() {
    assert!(
        Flavor::Rtf.read_rank() < Flavor::PlainText.read_rank(),
        "plain text is derivable from RTF, so RTF must be preferred"
    );
    assert!(Flavor::Html.read_rank() < Flavor::PlainText.read_rank());
    assert!(Flavor::Png.read_rank() < Flavor::Dib.read_rank());
    assert!(Flavor::FileList.read_rank() < Flavor::PlainText.read_rank());
}

#[test]
fn metadata_flavors_are_not_content() {
    assert!(!Flavor::DropEffect.is_content());
    assert!(Flavor::PlainText.is_content());
}

#[test]
fn legacy_pasteboard_twins_resolve_the_same_as_their_modern_uti() {
    // A live macOS pasteboard carries both spellings with byte-identical data.
    // These pairs are all present in corpus/macos/; resolving only one of a
    // pair makes a consumer's behaviour depend on which twin it happened to
    // read first.
    for (modern, legacy) in [
        ("public.utf8-plain-text", "NSStringPboardType"),
        ("public.html", "Apple HTML pasteboard type"),
        ("public.rtf", "NeXT Rich Text Format v1.0 pasteboard type"),
        ("public.tiff", "NeXT TIFF v4.0 pasteboard type"),
        ("public.png", "Apple PNG pasteboard type"),
        ("public.file-url", "NSFilenamesPboardType"),
        ("public.url", "Apple URL pasteboard type"),
    ] {
        assert_eq!(
            Flavor::from_uti(modern),
            Flavor::from_uti(legacy),
            "{legacy} carries the same bytes as {modern} and must resolve alike"
        );
    }
}

#[test]
fn the_ostype_wrapper_is_decoded_not_treated_as_opaque() {
    // corpus/macos/safari/ contains this exact identifier. The hex is four
    // ASCII bytes: 0x75743136 spells "ut16".
    assert_eq!(
        Flavor::from_uti("CorePasteboardFlavorType 0x75743136"),
        Flavor::PlainTextUtf16
    );
    assert_eq!(
        Flavor::from_uti("CorePasteboardFlavorType 0x54455854"), // "TEXT"
        Flavor::PlainText
    );
    assert_eq!(
        Flavor::from_uti("CorePasteboardFlavorType 0x504E4766"), // "PNGf"
        Flavor::Png
    );
}

#[test]
fn a_malformed_ostype_wrapper_stays_opaque_rather_than_guessing() {
    for bad in [
        "CorePasteboardFlavorType 0x7574313",   // 7 digits
        "CorePasteboardFlavorType 0x757431366", // 9 digits
        "CorePasteboardFlavorType 0xZZZZZZZZ",  // not hex
        "CorePasteboardFlavorType 0x00000000",  // valid hex, unknown code
        "CorePasteboardFlavorType",             // no hex at all
    ] {
        assert_eq!(
            Flavor::from_uti(bad),
            Flavor::Other(bad),
            "{bad} must round-trip verbatim rather than resolve to something wrong"
        );
    }
}

#[test]
fn utf16_text_is_not_the_same_flavor_as_utf8_text() {
    // A real pasteboard offers both side by side. Collapsing them means
    // handing UTF-16LE bytes to a UTF-8 decoder, which yields mojibake rather
    // than an error.
    assert_eq!(
        Flavor::from_uti("public.utf8-plain-text"),
        Flavor::PlainText
    );
    assert_eq!(
        Flavor::from_uti("public.utf16-external-plain-text"),
        Flavor::PlainTextUtf16
    );
    assert_ne!(Flavor::PlainText, Flavor::PlainTextUtf16);
    assert!(
        Flavor::PlainText.read_rank() < Flavor::PlainTextUtf16.read_rank(),
        "prefer the UTF-8 form when both are offered"
    );
}

#[test]
fn the_web_archive_outranks_the_markup_it_wraps() {
    assert_eq!(Flavor::from_uti("com.apple.webarchive"), Flavor::WebArchive);
    assert_eq!(
        Flavor::from_uti("Apple Web Archive pasteboard type"),
        Flavor::WebArchive
    );
    assert!(
        Flavor::WebArchive.read_rank() < Flavor::Html.read_rank(),
        "a web archive carries the HTML plus its subresources"
    );
}

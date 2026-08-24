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

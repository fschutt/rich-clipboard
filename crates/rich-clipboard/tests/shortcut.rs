//! The shortcut family: four file formats, one [`Link`].
//!
//! Reading these is the "drop a `.webloc` on the app and get structure back"
//! goal of `plan/PLAN.md` §4.10. They are files rather than clipboard flavors,
//! so they are reached directly rather than through `decode`.

#![cfg(feature = "shortcut")]

use rich_clipboard::{Link, LinkTarget};

fn fixture(krate: &str, name: &str) -> Vec<u8> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/");
    std::fs::read(format!("{root}{krate}/{name}")).expect("fixture")
}

#[test]
fn the_four_formats_are_told_apart_by_their_bytes_and_not_their_extension() {
    // The extension is a hint the sender chose; the bytes are not.
    let cases = [
        (
            fixture("rclip-url-file", "minimal-lf.bin"),
            "https://example.com/",
        ),
        (
            fixture("rclip-webloc", "inetloc-urlname.bin"),
            "https://www.rust-lang.org/",
        ),
        (
            fixture("rclip-desktop-entry", "link-simple.bin"),
            "https://example.com/",
        ),
        (
            fixture("rclip-bookmark", "url-and-filename.bin"),
            "file:///Users/example/Documents/report.pdf",
        ),
    ];
    for (bytes, expected) in cases {
        let link = Link::from_file(&bytes).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(link.target.as_str(), expected);
    }
}

#[test]
fn an_inetloc_title_comes_through_and_a_plain_webloc_has_none() {
    let inetloc = Link::from_webloc(&fixture("rclip-webloc", "inetloc-urlname.bin")).unwrap();
    assert_eq!(
        inetloc.title.as_deref(),
        Some("The Rust Programming Language")
    );

    let webloc = Link::from_webloc(&fixture("rclip-webloc", "finder-created.bin")).unwrap();
    assert_eq!(webloc.title, None);
}

#[test]
fn a_desktop_entry_keeps_its_name() {
    let link =
        Link::from_desktop_entry(&fixture("rclip-desktop-entry", "link-simple.bin")).unwrap();
    assert_eq!(link.title.as_deref(), Some("Example"));
}

#[test]
fn a_desktop_launcher_is_refused_rather_than_turned_into_a_destination() {
    // `Type=Application` describes a *program to run*, which is not a place.
    // Turning one into a `Link` is how a dropped `.desktop` file becomes an
    // execution.
    let bytes = b"[Desktop Entry]\nType=Application\nName=Terminal\nExec=xterm\n";
    assert!(Link::from_desktop_entry(bytes).is_err());
}

#[test]
fn a_windows_path_is_classified_as_a_path_and_not_as_a_one_letter_scheme() {
    // `C:\Users\me` is a syntactically valid RFC 3986 URI reference with the
    // scheme `C`. Checking for a colon first turns every Windows path on the
    // clipboard into a URL.
    assert_eq!(
        LinkTarget::classify(r"C:\Users\me"),
        LinkTarget::Path(r"C:\Users\me".into())
    );
    assert_eq!(
        LinkTarget::classify(r"\\server\share"),
        LinkTarget::Path(r"\\server\share".into())
    );
    assert_eq!(
        LinkTarget::classify("/home/me"),
        LinkTarget::Path("/home/me".into())
    );
    assert_eq!(
        LinkTarget::classify("https://example.com/"),
        LinkTarget::Url("https://example.com/".into())
    );
    assert_eq!(
        LinkTarget::classify("some-file.txt"),
        LinkTarget::Unresolved("some-file.txt".into())
    );
    assert_eq!(
        LinkTarget::classify(""),
        LinkTarget::Unresolved(String::new())
    );
}

#[test]
fn a_shortcut_file_that_is_none_of_the_four_is_refused_and_not_guessed_at() {
    assert!(Link::from_file(b"just some text").is_err());
}

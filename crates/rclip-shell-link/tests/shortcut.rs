//! The shortcut-family view of a `.lnk`: `ShellLink::target_candidates` and
//! `ShellLink::target`, driven by `corpus/synthetic/rclip-shell-link`.
//!
//! A separate file from `shell_link.rs` because it tests a separate question:
//! not "did the structure parse" but "what does this link say it points at",
//! which is the one thing `.url`, `.webloc`, `.desktop` and `text/uri-list`
//! also answer.

use rclip_shell_link::{ShellLink, ShortcutTarget, TargetSource, HEADER_SIZE};

fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/synthetic/rclip-shell-link/"
    );
    std::fs::read(format!("{p}{name}")).expect("fixture")
}

#[test]
fn a_drive_letter_link_info_yields_a_path_not_a_url() {
    let buf = fixture("full-featured.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    // The whole point of the classification order. `C:\Users\me\notes.txt` is a
    // syntactically valid RFC 3986 reference with the scheme `C`, so a parser
    // that tests for a colon first turns every Windows path into a URL.
    assert_eq!(
        link.target(),
        Some(ShortcutTarget::Path("C:\\Users\\me\\notes.txt"))
    );
}

#[test]
fn candidates_come_back_most_absolute_first() {
    let buf = fixture("full-featured.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    let got: Vec<_> = link
        .target_candidates()
        .map(|c| (c.source, c.text.as_ascii()))
        .collect();

    assert_eq!(
        got,
        vec![
            (
                TargetSource::LocalBasePath,
                Some("C:\\Users\\me\\notes.txt")
            ),
            // Both of these are UTF-16 in this fixture — `IsUnicode` is set —
            // so they are candidates a caller can decode and not strings that
            // can be borrowed. That is the whole reason the candidate carries a
            // `ShellStr` and not a `&str`.
            (TargetSource::EnvironmentPath, None),
            (TargetSource::RelativePath, None),
        ],
        "a link routinely names its target several times; the list is ordered \
         by how much each spelling means on another machine"
    );
}

#[test]
fn a_unc_net_name_is_a_path_too() {
    let buf = fixture("link-info-network.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    assert_eq!(
        link.target(),
        Some(ShortcutTarget::Path("\\\\fileserver\\public")),
        "the double-backslash test has to run before the scheme test"
    );

    let sources: Vec<_> = link.target_candidates().map(|c| c.source).collect();
    assert_eq!(
        sources,
        vec![TargetSource::NetName],
        "no VolumeIDAndLocalBasePath flag, so no LocalBasePath candidate"
    );
}

#[test]
fn an_environment_path_is_unresolved_because_nothing_here_expands_it() {
    let buf = fixture("with-extra-data.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    let env = link
        .target_candidates()
        .find(|c| c.source == TargetSource::EnvironmentPath)
        .expect("the fixture carries an EnvironmentVariableDataBlock");

    assert_eq!(
        env.text.to_string_lossy(),
        "%windir%\\system32\\cmd.exe",
        "the block's Unicode half wins, so this is the string on offer"
    );
    // `%windir%\...` is neither a URL nor a path until something expands it,
    // and expanding it is not a parser's job — so once it *can* be borrowed it
    // classifies as Unresolved.
    assert_eq!(
        ShortcutTarget::classify("%windir%\\system32\\cmd.exe"),
        ShortcutTarget::Unresolved("%windir%\\system32\\cmd.exe")
    );
}

#[test]
fn a_relative_path_is_unresolved() {
    let buf = fixture("ansi-string-data.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    assert_eq!(
        link.target(),
        Some(ShortcutTarget::Unresolved(".\\a.txt")),
        "relative to the .lnk file, whose location a parser does not know"
    );
    assert_eq!(
        link.target_candidates().next().map(|c| c.source),
        Some(TargetSource::RelativePath)
    );
}

#[test]
fn a_header_only_link_names_nothing() {
    let buf = fixture("minimal-header-only.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    assert_eq!(link.target_candidates().count(), 0);
    assert_eq!(link.target(), None);
}

#[test]
fn utf16_string_data_yields_a_candidate_but_no_borrowed_target() {
    let buf = fixture("with-string-data.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    let rel = link
        .target_candidates()
        .find(|c| c.source == TargetSource::RelativePath)
        .expect("IsUnicode is set, but the field is still there");

    assert!(rel.text.is_unicode());
    assert_eq!(
        rel.target(),
        None,
        "UTF-16 cannot be borrowed as a &str, and re-encoding allocates"
    );
    assert_eq!(
        rel.text.to_string_lossy(),
        ".\\notes.txt",
        "the bytes are reachable; only the zero-copy view is not"
    );
}

#[test]
fn an_ansi_field_above_ascii_is_not_borrowable_either() {
    let buf = fixture("ansi-string-data-cp1252.bin");
    let link = ShellLink::parse(&buf).expect("well formed");

    assert_eq!(
        link.target(),
        None,
        "the byte is Latin-1 `ü` in cp1252 and something else in cp1251; the \
         file does not say which, so there is no `&str` to hand back"
    );
    let rel = link
        .target_candidates()
        .next()
        .expect("a candidate all the same");
    assert!(!rel.text.is_unicode(), "an ANSI field, not a UTF-16 one");
    assert!(
        rel.text.as_bytes().iter().any(|b| *b > 0x7F),
        "and the byte that stops it being ASCII is right there for a caller \
         that knows the code page"
    );
}

#[test]
fn a_broken_link_info_costs_only_its_own_candidates() {
    // `link-info-too-small.bin` fails at `ShellLink::parse`, so the interesting
    // case is a link whose LinkInfo parses and whose offsets do not resolve.
    // Corrupting the local base path offset of the full fixture produces one.
    let mut buf = fixture("full-featured.bin");
    let idlist = ShellLink::parse(&buf)
        .unwrap()
        .target_id_list
        .expect("fixture has a target IDList")
        .wire_size();
    // MS-SHLLINK 2: LinkInfo follows the header and the LinkTargetIDList, and
    // LocalBasePathOffset is at +0x10 inside it.
    let field = HEADER_SIZE + idlist + 0x10;
    buf[field..field + 4].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());

    let link = ShellLink::parse(&buf).expect("the header still parses");
    let sources: Vec<_> = link.target_candidates().map(|c| c.source).collect();
    assert!(
        !sources.contains(&TargetSource::LocalBasePath),
        "an offset that does not resolve contributes nothing"
    );
    assert!(
        sources.contains(&TargetSource::RelativePath),
        "and costs no other candidate"
    );
}

#[test]
fn candidates_never_panic_on_a_corrupted_fixture() {
    for name in [
        "full-featured.bin",
        "link-info-network.bin",
        "ansi-string-data.bin",
        "with-string-data.bin",
    ] {
        let bytes = fixture(name);
        for i in 0..bytes.len() {
            for patch in [0x00u8, 0x0F, 0x7F, 0xFF] {
                let mut m = bytes.clone();
                m[i] = patch;
                if let Ok(link) = ShellLink::parse(&m) {
                    for c in link.target_candidates() {
                        let _ = c.target();
                        let _ = c.text.chars().count();
                    }
                    let _ = link.target();
                }
            }
        }
    }
}

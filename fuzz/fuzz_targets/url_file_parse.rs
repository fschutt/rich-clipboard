//! Windows `.url` InternetShortcut files — `rclip_url_file::parse`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_url_file::{parse, ShortcutTarget};

fuzz_target!(|data: &[u8]| {
    let Ok(f) = parse(data) else { return };

    // Parsing only validates UTF-8 and the INI shape; every field is resolved
    // lazily, so the accessors are where the work is.
    let mut sections = 0usize;
    for s in f.sections() {
        sections += 1;
        assert!(sections <= data.len() + 1, "section iterator did not advance");
        let _ = s.name();
        let mut entries = 0usize;
        for e in s.entries() {
            entries += 1;
            assert!(entries <= data.len() + 1, "entry iterator did not advance");
            let _ = (e.key, e.value, e.offset);
        }
    }

    let _ = f.internet_shortcut();
    let _ = f.url();
    let _ = f.require_url();
    let _ = f.url_ansi();
    let _ = f.url_wide();
    let _ = f.icon_file();
    let _ = f.icon_index();
    let _ = f.hotkey();
    let _ = f.show_command();
    let _ = f.modified();
    let _ = f.working_directory();
    let _ = f.id_list();

    // The target classification is what a caller acts on, so it must never
    // disagree with the URL it was derived from.
    match (f.target(), f.url()) {
        (Some(ShortcutTarget::Url(u)), Some(url)) => assert_eq!(u, url),
        (Some(_), Some(_)) | (None, None) | (None, Some(_)) => {}
        (Some(t), None) => panic!("classified a target with no URL: {t:?}"),
    }

    // `require_url` is documented as the fallible spelling of `url`.
    assert_eq!(f.url().is_some(), f.require_url().is_ok());
});

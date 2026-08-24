//! `CFSTR_FILEDESCRIPTORW` — `rclip_file_desc::FileGroupDescriptor::parse`.
//!
//! `cItems` is a `u32` straight off another process's clipboard and each
//! descriptor is 592 bytes, so `0xFFFFFFFF` is a 2.5 TiB read if the count is
//! not checked against the buffer before it is multiplied.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_file_desc::ansi::{FileGroupDescriptorA, DESCRIPTOR_A_LEN};
use rclip_file_desc::{Builder, FileGroupDescriptor, DESCRIPTOR_LEN, GROUP_HEADER_LEN};

fuzz_target!(|data: &[u8]| {
    // `CFSTR_FILEDESCRIPTORA` first, and before the wide parser's early
    // return, because otherwise it would only ever see the inputs the wide
    // parser already accepted. The two are *not* sniffed apart: which one a
    // payload is comes from the format name it was offered under, so the same
    // bytes are a legal input to both and a 332-byte stride reads a different
    // set of fields out of them than a 592-byte one.
    if let Ok(ansi) = FileGroupDescriptorA::parse(data) {
        assert_eq!(ansi.raw_items().len(), ansi.len() * DESCRIPTOR_A_LEN);
        assert!(GROUP_HEADER_LEN + ansi.raw_items().len() <= data.len());
        assert_eq!(ansi.is_empty(), ansi.len() == 0);
        let mut n = 0usize;
        for d in ansi.iter() {
            assert_eq!(ansi.get(n).unwrap(), d);
            n += 1;
            let name = d.file_name_ansi();
            // The name is whatever precedes the first NUL of a fixed 260-byte
            // field, so padding must not leak into it.
            // 260 and not 259: a name that fills the whole field with no room
            // for a terminator is tolerated by the reader and refused by the
            // writer, which is the same deliberate asymmetry the wide form
            // has. What must not happen is the field's zero padding leaking
            // into the name, which is the second assertion.
            assert!(name.len() <= 260, "the ANSI name outran its field");
            assert!(!name.contains(&0), "a NUL survived into the ANSI name");
            let _ = (d.file_size(), d.file_attributes(), d.is_directory());
            let _ = (d.claims_unicode(), d.is_shortcut(), d.wants_progress_ui());
            let _ = (d.clsid(), d.icon_size(), d.icon_position());
            let _ = (d.creation_time(), d.last_access_time(), d.last_write_time());
            let _ = d.raw();
        }
        assert_eq!(n, ansi.len());
        assert!(ansi.get(ansi.len()).is_none());
    }

    let Ok(group) = FileGroupDescriptor::parse(data) else {
        return;
    };

    // The count must have been validated against what is actually here.
    assert_eq!(group.raw_items().len(), group.len() * DESCRIPTOR_LEN);
    assert!(GROUP_HEADER_LEN + group.raw_items().len() <= data.len());
    assert_eq!(group.is_empty(), group.len() == 0);

    let mut b = Builder::new();
    let mut n = 0usize;
    for d in group.iter() {
        assert_eq!(group.get(n).unwrap(), d);
        n += 1;
        let _ = d.file_name_lossy();
        let _ = d.file_name_utf16();
        let _ = d.raw();
        let _ = d.file_size();
        // Round-trip feed: the writer takes exactly what the reader produced.
        // A name the parser accepted that the builder refuses is a genuine
        // disagreement between the two halves, except for the documented case
        // of a name with no room for a terminator, which the reader tolerates
        // and the writer will not emit.
        let _ = b.push_descriptor(&d);
    }
    assert_eq!(n, group.len());
    assert!(group.get(group.len()).is_none());

    // Round trip. Not byte-exact against the *input*, and deliberately so: the
    // reader truncates `cFileName` at its first NUL and the writer re-pads the
    // 260-unit field with zeros, so any bytes a producer left after the
    // terminator are dropped. `parse` likewise ignores trailing slack past the
    // last descriptor, because the payload arrives in an `HGLOBAL` and
    // `GlobalAlloc` rounds capacity up. The property that does hold is the
    // strong one on the value: re-serialising and re-parsing gives an equal
    // group, and serialising that again is byte-identical.
    if b.len() == group.len() {
        let bytes = b.finish();
        let back = FileGroupDescriptor::parse(&bytes).expect("our own output must parse");
        assert_eq!(back.len(), group.len());
        for (a, c) in group.iter().zip(back.iter()) {
            assert_eq!(a.raw(), c.raw(), "fixed fields changed in the round trip");
            assert_eq!(
                a.file_name_utf16(),
                c.file_name_utf16(),
                "file name changed in the round trip"
            );
        }
        let mut again = Builder::new();
        for d in back.iter() {
            again.push_descriptor(&d).expect("re-push what we just wrote");
        }
        assert_eq!(again.finish(), bytes, "serialization is not a fixed point");
    }
});

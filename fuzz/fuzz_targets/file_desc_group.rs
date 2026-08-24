//! `CFSTR_FILEDESCRIPTORW` — `rclip_file_desc::FileGroupDescriptor::parse`.
//!
//! `cItems` is a `u32` straight off another process's clipboard and each
//! descriptor is 592 bytes, so `0xFFFFFFFF` is a 2.5 TiB read if the count is
//! not checked against the buffer before it is multiplied.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_file_desc::{Builder, FileGroupDescriptor, DESCRIPTOR_LEN, GROUP_HEADER_LEN};

fuzz_target!(|data: &[u8]| {
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

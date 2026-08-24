//! One `FILEDESCRIPTORW` — `rclip_file_desc::FileDescriptor::parse`.
//!
//! A separate public entry point from the group parser: a caller that already
//! sliced a descriptor out of somewhere else calls this directly, so it has to
//! stand on its own.
//!
//! Coverage note: `parse` requires *exactly* 592 bytes and rejects everything
//! else with `BadLength` before reading a field. That is a length gate rather
//! than a magic number, and libFuzzer does find it (the seed corpus is built
//! from 592-byte slices, and length-preserving mutations keep it), but a naive
//! run spends most of its inputs on the early return.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_file_desc::{Builder, FileDescriptor, DESCRIPTOR_LEN};

fuzz_target!(|data: &[u8]| {
    let parsed = FileDescriptor::parse(data);
    if data.len() != DESCRIPTOR_LEN {
        assert!(parsed.is_err(), "parsed a descriptor of the wrong length");
        return;
    }
    // Every field is fixed-width, so a correctly sized buffer cannot fail.
    let d = parsed.expect("a 592-byte descriptor has no failure mode");

    let name = d.file_name_utf16();
    assert!(name.len() % 2 == 0, "UTF-16 name had an odd byte count");
    assert!(name.len() <= 260 * 2);
    let _ = d.file_name_lossy();
    let _ = (d.file_size(), d.raw());

    // Round trip. Byte-exact on the 72 fixed bytes -- `RawDescriptor` keeps
    // them verbatim whether or not their flag is set -- but only up to the NUL
    // in the name field, because the reader truncates there and the writer
    // re-pads with zeros. Bytes a producer left *after* the terminator are
    // dropped by design, so the property asserted is equality of the parsed
    // value plus byte equality of the part that is not lossy.
    let mut b = Builder::new();
    let Ok(()) = b.push_descriptor(&d) else {
        // A 260-unit name leaves no room for the terminator the writer insists
        // on. The reader tolerating it and the writer refusing to emit it is
        // deliberate, not a mismatch.
        return;
    };
    let bytes = b.finish();
    let round = &bytes[4..];
    assert_eq!(round.len(), DESCRIPTOR_LEN);
    assert_eq!(&round[..72], &data[..72], "fixed fields changed");
    let back = FileDescriptor::parse(round).expect("our own output must parse");
    assert_eq!(back.raw(), d.raw());
    assert_eq!(back.file_name_utf16(), name);
});

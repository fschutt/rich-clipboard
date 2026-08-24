//! `CF_DIB` / `CF_DIBV5` header validation — `rclip_dib::DibHeader::parse`.
//!
//! Split from `dib_decode` on purpose: the header carries every
//! attacker-controlled number in the format (width, height, bit count, palette
//! size, mask layout) and validating it is cheap, so a target that stops after
//! the header explores far more of that space per second than one that also
//! decodes megabytes of pixels for every input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_dib::DibHeader;

fuzz_target!(|data: &[u8]| {
    let Ok(h) = DibHeader::parse(data) else {
        return;
    };

    // Every derived offset is documented as in-bounds for *this* buffer once
    // parse has returned. If that is ever untrue, decode_into indexes out of
    // range, so assert it here where the failure is cheap to minimise.
    assert!(h.palette_offset() <= data.len());
    assert!(h.pixel_offset() <= data.len());
    assert!(
        h.pixel_offset() + h.image_bytes() <= data.len(),
        "pixel data runs past the end of the payload"
    );
    assert!(h.required_buffer_len() <= rclip_core::MAX_PIXELS as usize * 4);
    if h.is_palettised() {
        assert!(h.palette_entries() <= 256);
    }
    assert_eq!(
        h.required_buffer_len(),
        h.width() as usize * h.height() as usize * 4
    );

    let _ = (
        h.version(),
        h.is_top_down(),
        h.bit_count(),
        h.compression(),
        h.declared_size_image(),
        h.x_pels_per_meter(),
        h.y_pels_per_meter(),
        h.declared_clr_used(),
        h.clr_important(),
        h.color_space(),
        h.stride(),
    );
    let m = h.masks();
    let _ = m.uncovered();
});

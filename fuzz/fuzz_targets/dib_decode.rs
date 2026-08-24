//! `CF_DIB` / `CF_DIBV5` pixel decoding — `rclip_dib::decode`.
//!
//! Where `dib_header` stops at validation, this one walks every row: RLE is not
//! implemented but bit-field extraction, palette indexing and premultiplied
//! alpha all index derived offsets per pixel.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_dib::{decode, encode_v5, AlphaMode, DibHeader};

/// Above this, the round-trip is skipped. Not a guard on the *decode* — the
/// crate's own cap is `rclip_core::MAX_PIXELS`, and decoding right up to it is
/// exactly what this target is for — but `encode_v5` would allocate a second
/// buffer of the same size, and two 1 GiB allocations trip libFuzzer's RSS
/// limit on a payload the library is documented to accept.
const ROUND_TRIP_LIMIT: usize = 1 << 20;

fuzz_target!(|data: &[u8]| {
    // The header is parsed first anyway; doing it here lets the size of what
    // decode is about to allocate be known before the three decodes below,
    // rather than after three of them have run.
    let Ok(header) = DibHeader::parse(data) else {
        return;
    };

    // All three alpha policies. `Guess` inspects every pixel, so it is a
    // different code path and not just a different constant.
    for alpha in [AlphaMode::Straight, AlphaMode::Premultiplied, AlphaMode::Guess] {
        let Ok(img) = decode(data, alpha) else { continue };
        assert_eq!(img.width, header.width());
        assert_eq!(img.height, header.height());
        assert_eq!(img.pixels.len(), header.required_buffer_len());
        assert_eq!(img.pixels.len(), img.width as usize * img.height as usize * 4);

        if alpha != AlphaMode::Straight || img.pixels.len() > ROUND_TRIP_LIMIT {
            continue;
        }

        // Round trip. Lossy in one direction only: the encoder always writes a
        // 32bpp BITMAPV5HEADER, so a 1bpp or palettised source does not come
        // back as the same bytes. The RGBA pixels, which are the decoded value,
        // must survive exactly.
        let Ok(blob) = encode_v5(img.width, img.height, &img.pixels, AlphaMode::Straight) else {
            // Zero width or height: the encoder refuses what the decoder
            // accepts, because a zero-pixel DIB is not something to hand on.
            continue;
        };
        let back = decode(&blob, AlphaMode::Straight).expect("our own output must decode");
        assert_eq!(back.width, img.width);
        assert_eq!(back.height, img.height);
        assert_eq!(back.pixels, img.pixels, "RGBA did not survive the round trip");
        assert_eq!(
            encode_v5(back.width, back.height, &back.pixels, AlphaMode::Straight).unwrap(),
            blob
        );
    }
});

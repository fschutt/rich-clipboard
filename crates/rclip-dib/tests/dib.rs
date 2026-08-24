//! Integration tests for `rclip-dib`, driven by the synthetic corpus in
//! `corpus/synthetic/rclip-dib/`.
//!
//! Expected pixel values live here rather than in the `.json` sidecars so the
//! tests need no JSON parser; the sidecars carry the prose description and the
//! ok/error verdict, and [`sidecar_verdicts_match_reality`] checks that verdict
//! against what the parser actually does.

use rclip_core::ErrorKind;
use rclip_dib::{AlphaMode, DibHeader, HeaderVersion};

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/synthetic/rclip-dib/"
);

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{DIR}{name}.bin"))
        .unwrap_or_else(|e| panic!("fixture {name}.bin is missing or unreadable: {e}"))
}

/// Decode a fixture and return the RGBA8 bytes, top row first.
fn decode(name: &str, alpha: AlphaMode) -> Vec<u8> {
    let src = fixture(name);
    let header = DibHeader::parse(&src).unwrap_or_else(|e| panic!("{name} should parse: {e}"));
    let mut out = vec![0u8; header.required_buffer_len()];
    header
        .decode_into(&src, &mut out, alpha)
        .unwrap_or_else(|e| panic!("{name} should decode: {e}"));
    out
}

fn expect_err(name: &str, kind: ErrorKind) {
    let src = fixture(name);
    // Parse is where every declared size is checked, but go through the whole
    // pipeline anyway: a fixture that parses and then panics in the decoder is
    // exactly what these tests exist to catch.
    let err = match DibHeader::parse(&src) {
        Err(e) => e,
        Ok(header) => {
            let mut out = vec![0u8; header.required_buffer_len()];
            header
                .decode_into(&src, &mut out, AlphaMode::Straight)
                .expect_err(&format!("{name} must not decode successfully"))
        }
    };
    assert_eq!(
        err.kind,
        kind,
        "{name}: expected {}, got {} at byte {}",
        kind.as_str(),
        err.kind.as_str(),
        err.offset
    );
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

fn rgba(pixels: &[[u8; 4]]) -> Vec<u8> {
    pixels.iter().flatten().copied().collect()
}

// ---------------------------------------------------------------- layout ----

#[test]
fn bottom_up_24bpp_flips_rows_and_skips_stride_padding() {
    let header = DibHeader::parse(&fixture("24bpp-bottom-up-2x2")).unwrap();
    assert_eq!(header.version(), HeaderVersion::Info);
    assert!(
        !header.is_top_down(),
        "positive biHeight is the bottom-up default"
    );
    assert_eq!(
        header.stride(),
        8,
        "2 pixels x 3 bytes = 6, padded up to the next multiple of 4"
    );
    assert_eq!(header.required_buffer_len(), 2 * 2 * 4);

    assert_eq!(
        decode("24bpp-bottom-up-2x2", AlphaMode::Straight),
        rgba(&[RED, GREEN, BLUE, WHITE]),
        "the first row on the wire is the bottom row of the image"
    );
}

#[test]
fn top_down_negative_height_decodes_to_the_same_image() {
    let header = DibHeader::parse(&fixture("24bpp-top-down-2x2")).unwrap();
    assert!(header.is_top_down(), "negative biHeight means top-down");
    assert_eq!(
        header.height(),
        2,
        "height is reported as an absolute value"
    );

    assert_eq!(
        decode("24bpp-top-down-2x2", AlphaMode::Straight),
        decode("24bpp-bottom-up-2x2", AlphaMode::Straight),
        "the two fixtures are the same picture stored in opposite row orders"
    );
}

// ------------------------------------------------------------- bitfields ----

#[test]
fn bitfields_after_a_40_byte_header_do_not_shift_the_pixels() {
    let header = DibHeader::parse(&fixture("16bpp-bitfields-565-2x2")).unwrap();
    assert_eq!(
        header.pixel_offset(),
        52,
        "with a BITMAPINFOHEADER the three mask DWORDs sit between header and pixels"
    );
    assert_eq!(header.masks().red.mask(), 0xF800);
    assert_eq!(
        header.masks().green.bits(),
        6,
        "RGB565 gives green six bits"
    );

    assert_eq!(
        decode("16bpp-bitfields-565-2x2", AlphaMode::Straight),
        rgba(&[RED, GREEN, BLUE, WHITE]),
        "channels must rescale by rounding: 5-bit 31 is 255, not 248"
    );
}

#[test]
fn bitfields_inside_a_v4_header_do_not_shift_the_pixels() {
    let header = DibHeader::parse(&fixture("32bpp-v4-bitfields-2x1")).unwrap();
    assert_eq!(header.version(), HeaderVersion::V4);
    assert_eq!(
        header.pixel_offset(),
        108,
        "V4 stores the masks as header fields, so nothing follows the header"
    );

    assert_eq!(
        decode("32bpp-v4-bitfields-2x1", AlphaMode::Straight),
        rgba(&[RED, [0, 0, 255, 128]]),
    );
}

// ----------------------------------------------------------------- alpha ----

#[test]
fn v5_alpha_mask_is_honoured() {
    let header = DibHeader::parse(&fixture("32bpp-v5-alpha-2x2")).unwrap();
    assert_eq!(header.version(), HeaderVersion::V5);
    assert_eq!(header.masks().alpha.mask(), 0xFF00_0000);
    assert_eq!(header.color_space(), rclip_dib::LCS_SRGB);

    assert_eq!(
        decode("32bpp-v5-alpha-2x2", AlphaMode::Straight),
        rgba(&[
            [255, 0, 0, 255],
            [0, 255, 0, 128],
            [0, 0, 255, 64],
            [255, 255, 255, 0],
        ]),
    );
}

#[test]
fn premultiplied_mode_divides_alpha_back_out() {
    assert_eq!(
        decode("32bpp-v5-alpha-2x2", AlphaMode::Premultiplied),
        rgba(&[
            // a == 255: nothing to undo.
            [255, 0, 0, 255],
            // 255/128 and 255/64 both saturate: these are the "impossible"
            // pixels that prove the image was not premultiplied after all.
            [0, 255, 0, 128],
            [0, 0, 255, 64],
            // a == 0 carries no recoverable colour.
            [0, 0, 0, 0],
        ]),
    );
}

#[test]
fn guess_picks_straight_when_a_channel_exceeds_its_alpha() {
    // (255,255,255) over alpha 0 cannot be a premultiplied pixel, so the
    // heuristic must classify the whole image as straight and leave it alone.
    assert_eq!(
        decode("32bpp-v5-alpha-2x2", AlphaMode::Guess),
        decode("32bpp-v5-alpha-2x2", AlphaMode::Straight),
    );
}

#[test]
fn guess_picks_premultiplied_when_no_channel_exceeds_its_alpha() {
    // Half-intensity red at half alpha: consistent with premultiplication, so
    // the heuristic divides alpha out and recovers full red.
    let src = rclip_dib::encode_v5(1, 1, &[255, 0, 0, 128], AlphaMode::Premultiplied).unwrap();
    let decoded = rclip_dib::decode(&src, AlphaMode::Guess).unwrap();
    assert_eq!(
        decoded.pixels,
        vec![255, 0, 0, 128],
        "premultiplied 128,0,0 at alpha 128 must come back as straight 255,0,0"
    );
}

#[test]
fn thirty_two_bpp_info_header_alpha_needs_every_byte_set() {
    // A 40-byte header has no alpha channel; the fourth byte is undefined. It
    // only counts when nothing is zero.
    assert_eq!(
        decode("32bpp-info-alpha-all-set-2x1", AlphaMode::Straight),
        rgba(&[[255, 0, 0, 128], GREEN]),
        "no zero bytes, so the undefined byte is taken as alpha"
    );
    assert_eq!(
        decode("32bpp-info-alpha-zero-2x1", AlphaMode::Straight),
        rgba(&[RED, GREEN]),
        "one zero byte means it is filler, and the image is opaque"
    );
}

// --------------------------------------------------------------- palette ----

#[test]
fn palettised_8bpp_reads_bgra_entries_and_ignores_row_padding() {
    let header = DibHeader::parse(&fixture("8bpp-palette-3x2")).unwrap();
    assert!(header.is_palettised());
    assert_eq!(header.palette_entries(), 4, "biClrUsed was 4");
    assert_eq!(header.palette_offset(), 40);
    assert_eq!(header.pixel_offset(), 40 + 4 * 4);
    assert_eq!(header.stride(), 4, "3 index bytes padded to 4");

    // The pad byte is 0xFF, which is not a valid index into a four-entry
    // palette; if it were read this would be an error instead of an image.
    assert_eq!(
        decode("8bpp-palette-3x2", AlphaMode::Straight),
        rgba(&[RED, GREEN, BLUE, BLUE, GREEN, RED]),
    );
}

#[test]
fn palettised_4bpp_takes_the_high_nibble_first() {
    let header = DibHeader::parse(&fixture("4bpp-palette-5x2")).unwrap();
    assert_eq!(
        header.palette_entries(),
        16,
        "biClrUsed == 0 means 1 << biBitCount entries"
    );

    let grey = |i: u8| [i * 17, i * 17, i * 17, 255];
    assert_eq!(
        decode("4bpp-palette-5x2", AlphaMode::Straight),
        rgba(&[
            grey(1),
            grey(2),
            grey(3),
            grey(4),
            grey(5),
            grey(15),
            grey(14),
            grey(13),
            grey(12),
            grey(11),
        ]),
    );
}

#[test]
fn monochrome_1bpp_takes_the_most_significant_bit_first() {
    let header = DibHeader::parse(&fixture("1bpp-mono-9x2")).unwrap();
    assert_eq!(header.palette_entries(), 2);
    assert_eq!(header.stride(), 4, "9 bits is 2 bytes, padded to 4");

    assert_eq!(
        decode("1bpp-mono-9x2", AlphaMode::Straight),
        rgba(&[
            WHITE, BLACK, WHITE, BLACK, WHITE, BLACK, WHITE, BLACK, WHITE, //
            BLACK, WHITE, BLACK, WHITE, BLACK, WHITE, BLACK, WHITE, BLACK,
        ]),
    );
}

// ------------------------------------------------------------ malformed -----

#[test]
fn enormous_dimensions_are_rejected_before_anything_is_sized_by_them() {
    // 65536 x 65536 is 2^32 pixels, sixteen gigabytes of RGBA, declared by
    // forty bytes of input. This is the fixture that matters most.
    expect_err("huge-dimensions", ErrorKind::TooLarge);

    let src = fixture("huge-dimensions");
    let header = DibHeader::parse(&src);
    assert!(
        header.is_err(),
        "parse must fail rather than hand back a header with a 16 GiB buffer requirement"
    );
}

#[test]
fn max_pixels_is_the_exact_boundary() {
    // One pixel over MAX_PIXELS fails; the boundary itself is accepted, which
    // proves the check is a limit and not an accident of some smaller cap.
    let side = 1u32 << 14; // 16384 x 16384 == 2^28 == MAX_PIXELS
    assert_eq!(u64::from(side) * u64::from(side), rclip_core::MAX_PIXELS);

    let at_limit = info_header(side as i32, side as i32, 1, 32, 0);
    assert_eq!(
        DibHeader::parse(&at_limit).unwrap_err().kind,
        // The dimensions pass the pixel guard, so it fails later, on the
        // missing pixel data rather than on the declared size.
        ErrorKind::UnexpectedEof,
    );

    let over_limit = info_header(side as i32 + 1, side as i32, 1, 32, 0);
    assert_eq!(
        DibHeader::parse(&over_limit).unwrap_err().kind,
        ErrorKind::TooLarge,
    );
}

#[test]
fn ancient_and_unknown_header_sizes_are_unsupported() {
    expect_err("bitmapcoreheader-12", ErrorKind::Unsupported);
    expect_err("bad-header-size-64", ErrorKind::Unsupported);
}

#[test]
fn truncated_pixel_data_is_an_error_not_a_short_image() {
    expect_err("24bpp-truncated-2x2", ErrorKind::UnexpectedEof);
}

#[test]
fn rejected_headers() {
    // biPlanes must be 1; any other value implies a planar layout that would
    // decode as scrambled colour rather than fail.
    assert_eq!(
        DibHeader::parse(&info_header(2, 2, 3, 24, 0))
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
    // Zero height has no rows and no orientation.
    assert_eq!(
        DibHeader::parse(&info_header(2, 0, 1, 24, 0))
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
    // Negative width is not the mirror of negative height: only height's sign
    // is meaningful.
    assert_eq!(
        DibHeader::parse(&info_header(-2, 2, 1, 24, 0))
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
    // 2 bpp exists only on Windows CE.
    assert_eq!(
        DibHeader::parse(&info_header(2, 2, 1, 2, 0))
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
    // BI_JPEG and BI_PNG are an embedded JPEG or PNG in a DIB wrapper, and are
    // permanently out of scope for this crate rather than merely unwritten.
    for comp in [4u32, 5] {
        assert_eq!(
            DibHeader::parse(&info_header(2, 2, 1, 8, comp))
                .unwrap_err()
                .kind,
            ErrorKind::Unsupported,
            "compression {comp} should be reported as unsupported"
        );
    }
    // BI_RLE8 at 8 bpp is now decoded, so a bare header with no run stream at
    // all fails on the missing pixel data rather than on the compression.
    assert_eq!(
        DibHeader::parse(&info_header(2, 2, 1, 8, 1))
            .unwrap_err()
            .kind,
        ErrorKind::BadOffset,
        "a header with no palette and no runs stops short of the pixel offset"
    );
    // A FOURCC in biCompression means this is a DirectShow video format.
    assert_eq!(
        DibHeader::parse(&info_header(2, 2, 1, 24, 0x5659_5559))
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
}

#[test]
fn bitfield_masks_are_validated() {
    let with_masks = |r: u32, g: u32, b: u32, bpp: u16| {
        let mut v = info_header(2, 1, 1, bpp, 3);
        v.splice(40..40, [r, g, b].iter().flat_map(|m| m.to_le_bytes()));
        v
    };

    // A non-contiguous mask has no shift-and-scale that reproduces it, so
    // guessing one would emit wrong colours silently.
    assert_eq!(
        DibHeader::parse(&with_masks(0x0000_F00F, 0x0000_03E0, 0x0000_001F, 16))
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
    // A mask that addresses bits outside the pixel.
    assert_eq!(
        DibHeader::parse(&with_masks(0x00FF_0000, 0x0000_03E0, 0x0000_001F, 16))
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
    // A zero colour mask cannot produce a colour.
    assert_eq!(
        DibHeader::parse(&with_masks(0x0000_7C00, 0, 0x0000_001F, 16))
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
    // BI_BITFIELDS is only defined for 16 and 32 bpp.
    assert_eq!(
        DibHeader::parse(&with_masks(0xE0, 0x1C, 0x03, 8))
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
}

#[test]
fn no_input_prefix_panics() {
    // Not a fuzzer, but it is the cheap half of one: every truncation of every
    // fixture must come back as an error, never as a panic.
    for name in fixture_names() {
        let src = fixture(&name);
        for cut in 0..src.len() {
            let prefix = &src[..cut];
            if let Ok(header) = DibHeader::parse(prefix) {
                let mut out = vec![0u8; header.required_buffer_len()];
                let _ = header.decode_into(prefix, &mut out, AlphaMode::Guess);
            }
        }
    }
}

// ------------------------------------------------------------ decode API ----

#[test]
fn decode_into_rejects_a_short_buffer_and_a_foreign_buffer() {
    let src = fixture("24bpp-bottom-up-2x2");
    let header = DibHeader::parse(&src).unwrap();

    let mut short = vec![0u8; header.required_buffer_len() - 1];
    assert_eq!(
        header
            .decode_into(&src, &mut short, AlphaMode::Straight)
            .unwrap_err()
            .kind,
        ErrorKind::BadLength
    );

    // Offsets were validated against `src` and no other buffer.
    let other = fixture("32bpp-v5-alpha-2x2");
    let mut out = vec![0u8; header.required_buffer_len()];
    assert_eq!(
        header
            .decode_into(&other, &mut out, AlphaMode::Straight)
            .unwrap_err()
            .kind,
        ErrorKind::BadMagic
    );
}

#[test]
fn free_functions_agree_with_the_header_api() {
    let src = fixture("32bpp-v5-alpha-2x2");
    let owned = rclip_dib::decode(&src, AlphaMode::Straight).unwrap();
    assert_eq!((owned.width, owned.height), (2, 2));

    let mut out = vec![0u8; owned.pixels.len()];
    let header = rclip_dib::decode_into(&src, &mut out, AlphaMode::Straight).unwrap();
    assert_eq!(header.version(), HeaderVersion::V5);
    assert_eq!(out, owned.pixels);
}

// ---------------------------------------------------------------- encode ----

#[test]
fn round_trip_straight_alpha() {
    let width = 3u32;
    let height = 2u32;
    let pixels = rgba(&[
        [255, 0, 0, 255],
        [0, 255, 0, 200],
        [0, 0, 255, 1],
        [10, 20, 30, 40],
        [255, 255, 255, 0],
        [0, 0, 0, 255],
    ]);

    let encoded = rclip_dib::encode_v5(width, height, &pixels, AlphaMode::Straight).unwrap();
    assert_eq!(
        encoded.len(),
        rclip_dib::encoded_v5_len(width, height).unwrap()
    );

    let header = DibHeader::parse(&encoded).unwrap();
    assert_eq!(header.version(), HeaderVersion::V5);
    assert_eq!(header.bit_count(), 32);
    assert_eq!(header.compression(), rclip_dib::BI_BITFIELDS);
    assert_eq!(header.masks().alpha.mask(), 0xFF00_0000);
    assert!(
        !header.is_top_down(),
        "bottom-up is the default more third-party readers get right"
    );

    let decoded = rclip_dib::decode(&encoded, AlphaMode::Straight).unwrap();
    assert_eq!((decoded.width, decoded.height), (width, height));
    assert_eq!(
        decoded.pixels, pixels,
        "straight alpha must survive the round trip byte for byte"
    );
}

#[test]
fn round_trip_premultiplied_alpha() {
    // Colour channels at or below alpha, so premultiplying is lossy but the
    // recovered values should land within one level of the originals.
    let pixels = rgba(&[[200, 100, 50, 255], [80, 40, 20, 128], [0, 0, 0, 0]]);
    let encoded = rclip_dib::encode_v5(3, 1, &pixels, AlphaMode::Premultiplied).unwrap();
    let decoded = rclip_dib::decode(&encoded, AlphaMode::Premultiplied).unwrap();

    for (i, (got, want)) in decoded.pixels.iter().zip(pixels.iter()).enumerate() {
        assert!(
            got.abs_diff(*want) <= 1,
            "byte {i}: premultiply/unpremultiply drifted from {want} to {got}"
        );
    }
}

#[test]
fn encode_into_a_borrowed_buffer_matches_the_owned_form() {
    let pixels = rgba(&[RED, GREEN, BLUE, WHITE]);
    let mut buf = vec![0u8; rclip_dib::encoded_v5_len(2, 2).unwrap()];
    let n = rclip_dib::encode_v5_into(2, 2, &pixels, AlphaMode::Straight, &mut buf).unwrap();
    assert_eq!(n, buf.len());
    assert_eq!(
        buf,
        rclip_dib::encode_v5(2, 2, &pixels, AlphaMode::Straight).unwrap()
    );
}

#[test]
fn encode_rejects_guess_and_bad_sizes() {
    let pixels = [0u8; 4];
    // There is nothing to guess about pixels the caller already owns.
    assert_eq!(
        rclip_dib::encode_v5(1, 1, &pixels, AlphaMode::Guess)
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        rclip_dib::encode_v5(0, 1, &pixels, AlphaMode::Straight)
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
    assert_eq!(
        rclip_dib::encoded_v5_len(1 << 20, 1 << 20)
            .unwrap_err()
            .kind,
        ErrorKind::TooLarge
    );
    // Two pixels declared, one supplied.
    assert_eq!(
        rclip_dib::encode_v5(2, 1, &pixels, AlphaMode::Straight)
            .unwrap_err()
            .kind,
        ErrorKind::BadLength
    );
}

// ------------------------------------------------------------------- RLE ----

/// Pixels no run covers. RLE is the one DIB encoding that can leave a hole, and
/// a hole is not palette entry zero — that is a real colour a real run could
/// have written.
const HOLE: [u8; 4] = [0, 0, 0, 0];

#[test]
fn rle8_walks_encoded_absolute_and_delta_runs() {
    let src = fixture("8bpp-rle8-4x3");
    let header = DibHeader::parse(&src).unwrap();
    assert!(header.is_rle());
    assert_eq!(header.compression(), rclip_dib::BI_RLE8);
    assert_eq!((header.width(), header.height()), (4, 3));
    assert_eq!(
        header.stride(),
        0,
        "a compressed row has no fixed stride, and reporting the decoded one \
         would let packed-path arithmetic produce a plausible wrong answer"
    );
    assert_eq!(
        header.image_bytes(),
        22,
        "for RLE, image_bytes is the length of the compressed stream"
    );

    assert_eq!(
        decode("8bpp-rle8-4x3", AlphaMode::Straight),
        rgba(&[
            // Top row: a delta skipped the first two pixels entirely.
            HOLE, HOLE, GREEN, GREEN,
            // Middle row: an absolute run of three, then a one-pixel run.
            GREEN, BLUE, GREEN, BLUE,
            // Bottom row on the wire is the first: a single run of four.
            RED, RED, RED, RED,
        ])
    );
}

#[test]
fn rle4_alternates_nibbles_and_pads_absolute_runs_to_a_word() {
    let src = fixture("4bpp-rle4-6x2");
    let header = DibHeader::parse(&src).unwrap();
    assert!(header.is_rle());
    assert_eq!(header.compression(), rclip_dib::BI_RLE4);
    assert_eq!(header.bit_count(), 4);

    assert_eq!(
        decode("4bpp-rle4-6x2", AlphaMode::Straight),
        rgba(&[
            // Absolute run of five (3 packed bytes + a pad byte), then one more.
            BLUE, BLACK, BLUE, BLACK, BLUE, GREEN,
            // Encoded run of six from the two nibbles of 0x12, high nibble first.
            RED, GREEN, RED, GREEN, RED, GREEN,
        ])
    );
}

#[test]
fn a_delta_that_leaves_the_bitmap_is_rejected() {
    // Unsigned offsets, so the *read* cursor only moves forward — but the rows
    // are bottom-up, so advancing y walks backwards through the output buffer.
    expect_err("rle8-delta-past-top", ErrorKind::Malformed);
}

#[test]
fn an_absolute_run_longer_than_the_stream_is_eof_not_a_read_past_the_end() {
    expect_err("rle8-absolute-past-end", ErrorKind::UnexpectedEof);
}

#[test]
fn a_run_past_the_last_row_or_the_row_end_is_rejected_before_it_is_written() {
    expect_err("rle8-run-past-rows", ErrorKind::Malformed);
    expect_err("rle8-run-past-width", ErrorKind::Malformed);
}

#[test]
fn an_rle_run_naming_a_palette_entry_that_is_not_there_is_rejected() {
    expect_err("rle8-palette-index-past-end", ErrorKind::Malformed);
}

#[test]
fn a_compressed_top_down_bitmap_is_a_contradiction() {
    expect_err("rle8-top-down", ErrorKind::Malformed);
}

#[test]
fn the_rle_variant_must_match_the_bit_count() {
    expect_err("rle4-bit-count-mismatch", ErrorKind::Malformed);
}

#[test]
fn a_truncated_rle_stream_stops_rather_than_looping() {
    // Every path through the run loop consumes at least the two bytes it reads
    // at the top, so a stream that ends mid-grammar terminates. Feed it every
    // prefix of a good fixture and require each one to answer.
    let src = fixture("8bpp-rle8-4x3");
    let body = 40 + 16;
    for len in body..=src.len() {
        let mut truncated = src[..len].to_vec();
        // biSizeImage names a stream longer than what is left; zero it so the
        // header reads "the rest of the payload" and the run loop, not the
        // length check, is what is under test.
        truncated[20..24].copy_from_slice(&0u32.to_le_bytes());
        let header = DibHeader::parse(&truncated).expect("header is intact");
        let mut out = vec![0u8; header.required_buffer_len()];
        // Ok or Err, but never a hang and never a panic.
        let _ = header.decode_into(&truncated, &mut out, AlphaMode::Guess);
    }
}

#[test]
fn a_declared_size_image_longer_than_the_payload_is_eof() {
    let mut src = fixture("8bpp-rle8-4x3");
    src[20..24].copy_from_slice(&9999u32.to_le_bytes());
    assert_eq!(
        DibHeader::parse(&src).unwrap_err().kind,
        ErrorKind::UnexpectedEof,
        "biSizeImage is the only statement of the stream's length, so an \
         impossible one is a truncated payload rather than something to clamp"
    );
}

#[test]
fn random_rle_streams_terminate_and_never_panic() {
    // A seeded stand-in for the fuzzer, so the bound on the run loop is checked
    // against inputs nobody chose. Deterministic: the same 20k streams every
    // run, so a failure is reproducible from the seed alone.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    // Both grammars: RLE4 packs two pixels per absolute byte and alternates
    // nibbles in an encoded run, so it is a different walk and not a constant.
    let heads: [Vec<u8>; 2] = [
        fixture("8bpp-rle8-4x3")[..40 + 16].to_vec(),
        fixture("4bpp-rle4-6x2")[..40 + 16].to_vec(),
    ];

    for case in 0..20_000u32 {
        let len = 2 + (next() % 60) as usize;
        let mut src = heads[(case % 2) as usize].clone();
        // biSizeImage = 0, i.e. "the stream is the rest of the payload", so the
        // run loop and not the length check is what is under test.
        src[20..24].copy_from_slice(&0u32.to_le_bytes());
        for _ in 0..len {
            src.push((next() >> 24) as u8);
        }
        let header = DibHeader::parse(&src).expect("the header is intact");
        assert!(header.is_rle());
        let mut out = vec![0u8; header.required_buffer_len()];
        // Ok or Err, never a panic and never a loop: every iteration consumes
        // at least the two bytes it reads, so the stream is the bound.
        let _ = header
            .decode_into(&src, &mut out, AlphaMode::Guess)
            .map_err(|e| assert!(e.offset <= src.len(), "case {case}: offset out of range"));
    }
}

// ------------------------------------------------- colour management --------

#[test]
fn v5_endpoints_and_gamma_are_reported_verbatim() {
    let header = DibHeader::parse(&fixture("32bpp-v5-calibrated-endpoints-2x1")).unwrap();
    assert_eq!(header.version(), HeaderVersion::V5);
    assert_eq!(header.color_space(), rclip_dib::LCS_CALIBRATED_RGB);
    assert!(header.is_calibrated());

    let e = header.endpoints().expect("V5 carries a CIEXYZTRIPLE");
    // FXPT2DOT30: two integer bits, thirty fraction bits.
    let (rx, ry, rz) = e.red.to_f32();
    assert!((rx - 0.6400).abs() < 1e-6, "ciexyzRed.X was {rx}");
    assert!((ry - 0.3300).abs() < 1e-6);
    assert!((rz - 0.0300).abs() < 1e-6);
    assert!((e.green.to_f32().1 - 0.6000).abs() < 1e-6);
    assert!((e.blue.to_f32().2 - 0.7900).abs() < 1e-6);
    assert_eq!(
        e.red.x, 687194767,
        "the raw field, not the float, is stored"
    );

    let g = header.gamma().expect("V5 carries three gamma DWORDs");
    assert_eq!(g.red, 0x0002_3333, "unsigned 16.16");
    assert!((g.to_f32().0 - 2.2).abs() < 1e-4);
}

#[test]
fn a_40_byte_header_has_no_colour_management_block_to_report() {
    let header = DibHeader::parse(&fixture("24bpp-bottom-up-2x2")).unwrap();
    assert_eq!(header.endpoints(), None);
    assert_eq!(header.gamma(), None);
    assert!(
        !header.is_calibrated(),
        "LCS_CALIBRATED_RGB is the default value of a field this header does not have"
    );
}

#[test]
fn the_colour_management_block_does_not_move_the_pixels() {
    // The whole point of reporting rather than applying: gamma 2.2 and a set of
    // primaries must not change a single decoded byte.
    let calibrated = decode("32bpp-v5-calibrated-endpoints-2x1", AlphaMode::Straight);
    assert_eq!(calibrated, rgba(&[[255, 0, 0, 255], [0, 0, 255, 128]]));
}

// -------------------------------------------------------- CF_DIB encoder ----

#[test]
fn cf_dib_encodes_a_40_byte_24bpp_bi_rgb_payload() {
    let pixels = rgba(&[RED, GREEN, BLUE, WHITE]);
    let encoded = rclip_dib::encode_dib(2, 2, &pixels, rclip_dib::Flatten::Discard).unwrap();

    let header = DibHeader::parse(&encoded).unwrap();
    assert_eq!(
        header.version(),
        HeaderVersion::Info,
        "CF_DIB, not CF_DIBV5"
    );
    assert_eq!(header.bit_count(), 24);
    assert_eq!(header.compression(), rclip_dib::BI_RGB);
    assert!(
        !header.is_top_down(),
        "bottom-up is what the legacy consumers CF_DIB exists for expect"
    );
    assert!(
        !header.masks().alpha.is_present(),
        "a BITMAPINFOHEADER has no alpha mask field at all"
    );
    assert_eq!(
        header.endpoints(),
        None,
        "a BITMAPINFOHEADER has nowhere to put a colour space"
    );

    let decoded = rclip_dib::decode(&encoded, AlphaMode::Straight).unwrap();
    assert_eq!((decoded.width, decoded.height), (2, 2));
    assert_eq!(
        decoded.pixels, pixels,
        "opaque input must survive the round trip byte for byte"
    );
}

#[test]
fn cf_dib_pads_odd_rows_to_a_four_byte_stride() {
    // 3 pixels x 3 bytes = 9 bytes of colour, 12 on the wire. The classic DIB
    // off-by-one: get it wrong and the image shears one pixel per row.
    let pixels = rgba(&[RED, GREEN, BLUE, WHITE, BLACK, RED]);
    let len = rclip_dib::encoded_dib_len(3, 2).unwrap();
    assert_eq!(len, 40 + 12 * 2);

    let encoded = rclip_dib::encode_dib(3, 2, &pixels, rclip_dib::Flatten::Discard).unwrap();
    assert_eq!(encoded.len(), len);
    let header = DibHeader::parse(&encoded).unwrap();
    assert_eq!(header.stride(), 12);
    assert_eq!(
        &encoded[40 + 9..40 + 12],
        &[0, 0, 0],
        "the pad is written, not left as whatever the caller's buffer held"
    );

    assert_eq!(
        rclip_dib::decode(&encoded, AlphaMode::Straight)
            .unwrap()
            .pixels,
        pixels
    );
}

#[test]
fn cf_dib_flattens_alpha_the_way_the_caller_asked() {
    // Half-transparent pure red over white.
    let pixels = rgba(&[[255, 0, 0, 128]]);

    let discarded = rclip_dib::encode_dib(1, 1, &pixels, rclip_dib::Flatten::Discard).unwrap();
    assert_eq!(
        rclip_dib::decode(&discarded, AlphaMode::Straight)
            .unwrap()
            .pixels,
        rgba(&[[255, 0, 0, 255]]),
        "Discard keeps the colour a straight-alpha pixel actually holds"
    );

    let over = rclip_dib::encode_dib(1, 1, &pixels, rclip_dib::Flatten::OVER_WHITE).unwrap();
    let got = rclip_dib::decode(&over, AlphaMode::Straight)
        .unwrap()
        .pixels;
    assert_eq!(got[0], 255, "red stays at full over a white background");
    assert!(
        got[1].abs_diff(127) <= 1 && got[2].abs_diff(127) <= 1,
        "green and blue should come halfway up to white, got {got:?}"
    );
    assert_eq!(
        got[3], 255,
        "CF_DIB has no alpha channel to be transparent in"
    );
}

#[test]
fn cf_dib_compositing_is_exact_at_both_ends() {
    let opaque = rgba(&[[10, 200, 90, 255]]);
    assert_eq!(
        rclip_dib::decode(
            &rclip_dib::encode_dib(1, 1, &opaque, rclip_dib::Flatten::OVER_WHITE).unwrap(),
            AlphaMode::Straight
        )
        .unwrap()
        .pixels,
        opaque,
        "alpha 255 must land exactly on the source colour, not one level off"
    );

    let clear = rgba(&[[10, 200, 90, 0]]);
    assert_eq!(
        rclip_dib::decode(
            &rclip_dib::encode_dib(1, 1, &clear, rclip_dib::Flatten::Over([1, 2, 3])).unwrap(),
            AlphaMode::Straight
        )
        .unwrap()
        .pixels,
        rgba(&[[1, 2, 3, 255]]),
        "alpha 0 must land exactly on the background"
    );
}

#[test]
fn cf_dib_encode_into_a_borrowed_buffer_matches_the_owned_form() {
    let pixels = rgba(&[RED, GREEN, BLUE, WHITE]);
    let mut buf = vec![0xAAu8; rclip_dib::encoded_dib_len(2, 2).unwrap()];
    let n =
        rclip_dib::encode_dib_into(2, 2, &pixels, rclip_dib::Flatten::Discard, &mut buf).unwrap();
    assert_eq!(n, buf.len());
    assert_eq!(
        buf,
        rclip_dib::encode_dib(2, 2, &pixels, rclip_dib::Flatten::Discard).unwrap()
    );
}

#[test]
fn cf_dib_encode_rejects_bad_sizes() {
    let pixels = [0u8; 4];
    assert_eq!(
        rclip_dib::encode_dib(0, 1, &pixels, rclip_dib::Flatten::Discard)
            .unwrap_err()
            .kind,
        ErrorKind::Malformed
    );
    assert_eq!(
        rclip_dib::encoded_dib_len(1 << 20, 1 << 20)
            .unwrap_err()
            .kind,
        ErrorKind::TooLarge
    );
    // Two pixels declared, one supplied.
    assert_eq!(
        rclip_dib::encode_dib(2, 1, &pixels, rclip_dib::Flatten::Discard)
            .unwrap_err()
            .kind,
        ErrorKind::BadLength
    );
    // A destination shorter than the payload.
    let mut small = [0u8; 8];
    assert_eq!(
        rclip_dib::encode_dib_into(1, 1, &pixels, rclip_dib::Flatten::Discard, &mut small)
            .unwrap_err()
            .kind,
        ErrorKind::BadLength
    );
}

// --------------------------------------------------------------- corpus -----

#[test]
fn sidecar_verdicts_match_reality() {
    let mut seen = 0usize;
    for name in fixture_names() {
        let sidecar = std::fs::read_to_string(format!("{DIR}{name}.json"))
            .unwrap_or_else(|e| panic!("{name}.bin has no .json sidecar: {e}"));
        assert!(
            sidecar.contains("\"origin\": \"synthetic\""),
            "{name}: Phase-0 fixtures are hand-built, so origin must be synthetic"
        );

        let expect_ok = if sidecar.contains("\"expect\": \"ok\"") {
            true
        } else if sidecar.contains("\"expect\": \"error\"") {
            false
        } else {
            panic!("{name}: sidecar must declare \"expect\" as \"ok\" or \"error\"");
        };

        let src = fixture(&name);
        let result = DibHeader::parse(&src).and_then(|h| {
            let mut out = vec![0u8; h.required_buffer_len()];
            h.decode_into(&src, &mut out, AlphaMode::Guess)
        });
        assert_eq!(
            result.is_ok(),
            expect_ok,
            "{name}: sidecar says expect={}, parser says {result:?}",
            if expect_ok { "ok" } else { "error" }
        );
        seen += 1;
    }
    assert!(
        seen >= 22,
        "expected the full synthetic corpus, found {seen}"
    );
}

fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(DIR)
        .expect("corpus/synthetic/rclip-dib must exist")
        .map(|e| e.expect("readable dir entry").file_name())
        .filter_map(|n| {
            n.to_str()
                .and_then(|s| s.strip_suffix(".bin"))
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

/// A bare 40-byte `BITMAPINFOHEADER` with no palette and no pixel data, for
/// the cases where the malformation is in the header itself.
fn info_header(width: i32, height: i32, planes: u16, bpp: u16, compression: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(40);
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&width.to_le_bytes());
    v.extend_from_slice(&height.to_le_bytes());
    v.extend_from_slice(&planes.to_le_bytes());
    v.extend_from_slice(&bpp.to_le_bytes());
    v.extend_from_slice(&compression.to_le_bytes());
    v.extend_from_slice(&[0u8; 20]); // sizeImage, x/y pels, clrUsed, clrImportant
    assert_eq!(v.len(), 40);
    v
}

// ------------------------------------------------------------ mask math -----

#[test]
fn channel_masks_rescale_by_rounding_not_by_shifting() {
    use rclip_dib::ChannelMask;

    let five = ChannelMask::new(0x0000_7C00);
    assert_eq!(five.bits(), 5);
    assert_eq!(
        five.extract(0x0000_7C00),
        255,
        "the 5-bit maximum must reach full white; a left shift would stop at 248"
    );
    assert_eq!(five.extract(0x0000_4000), 132, "16 of 31 rounds to 132");
    assert_eq!(five.extract(0), 0);

    let one = ChannelMask::new(0x0000_8000);
    assert_eq!(
        one.extract(0x0000_8000),
        255,
        "ARGB1555 alpha is all or nothing"
    );
    assert_eq!(one.extract(0), 0);

    let eight = ChannelMask::new(0xFF00_0000);
    assert_eq!(
        eight.extract(0xAB00_0000),
        0xAB,
        "8 bits pass through unscaled"
    );

    assert_eq!(ChannelMask::NONE.extract(u32::MAX), 0);
    assert!(!ChannelMask::NONE.is_present());
}

#[test]
fn header_sizes_map_to_versions() {
    for (size, version) in [
        (rclip_dib::BITMAPINFOHEADER_SIZE, HeaderVersion::Info),
        (rclip_dib::BITMAPV2INFOHEADER_SIZE, HeaderVersion::V2),
        (rclip_dib::BITMAPV3INFOHEADER_SIZE, HeaderVersion::V3),
        (rclip_dib::BITMAPV4HEADER_SIZE, HeaderVersion::V4),
        (rclip_dib::BITMAPV5HEADER_SIZE, HeaderVersion::V5),
    ] {
        assert_eq!(HeaderVersion::from_size(size).unwrap(), version);
        assert_eq!(version.size(), size, "from_size and size must be inverses");
    }
    assert!(HeaderVersion::from_size(rclip_dib::BITMAPCOREHEADER_SIZE).is_err());
}

#[test]
fn undocumented_56_byte_header_carries_an_alpha_mask_under_bi_rgb() {
    // Photoshop and older GIMP builds write a 56-byte header: BITMAPINFOHEADER
    // plus four masks, with biCompression left at BI_RGB. Unlike the colour
    // masks, the alpha mask is never qualified with "valid only if
    // BI_BITFIELDS", so it applies here.
    let mut v = Vec::new();
    v.extend_from_slice(&56u32.to_le_bytes());
    v.extend_from_slice(&1i32.to_le_bytes());
    v.extend_from_slice(&1i32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    v.extend_from_slice(&[0u8; 20]);
    v.extend_from_slice(&0x00FF_0000u32.to_le_bytes());
    v.extend_from_slice(&0x0000_FF00u32.to_le_bytes());
    v.extend_from_slice(&0x0000_00FFu32.to_le_bytes());
    v.extend_from_slice(&0xFF00_0000u32.to_le_bytes());
    v.extend_from_slice(&[0x40, 0x80, 0xC0, 0x20]); // one pixel: B, G, R, A

    let header = DibHeader::parse(&v).unwrap();
    assert_eq!(header.version(), HeaderVersion::V3);
    assert_eq!(
        header.pixel_offset(),
        56,
        "the masks are header fields, not a suffix"
    );
    assert_eq!(header.masks().alpha.mask(), 0xFF00_0000);

    let mut out = [0u8; 4];
    header
        .decode_into(&v, &mut out, AlphaMode::Straight)
        .unwrap();
    assert_eq!(out, [0xC0, 0x80, 0x40, 0x20]);
}

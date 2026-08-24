//! `CF_DIB` and `CF_DIBV5` — Windows packed device-independent bitmaps.
//!
//! These are the two image formats on the Windows clipboard that no general
//! image library serves well, because a clipboard DIB is *packed*: it has no
//! 14-byte `BITMAPFILEHEADER`, so the payload begins at the information header
//! and there is no `BM` magic, no file size and no explicit offset to the
//! pixels. Everything about the layout has to be derived from the header's own
//! fields, and the first `u32` — the header size, 40 / 108 / 124 — is the only
//! thing that says which header it is.
//!
//! ```
//! use rclip_dib::{AlphaMode, DibHeader};
//!
//! // A 1x1 24-bpp CF_DIB: 40-byte BITMAPINFOHEADER, then one 4-byte row
//! // (three colour bytes plus one pad byte to reach the 4-byte stride).
//! let payload: &[u8] = &[
//!     40, 0, 0, 0, // biSize      = 40 (BITMAPINFOHEADER)
//!     1, 0, 0, 0,  // biWidth     = 1
//!     1, 0, 0, 0,  // biHeight    = 1 (positive: bottom-up)
//!     1, 0,        // biPlanes    = 1
//!     24, 0,       // biBitCount  = 24
//!     0, 0, 0, 0,  // biCompression = BI_RGB
//!     0, 0, 0, 0,  // biSizeImage = 0, routinely omitted for BI_RGB
//!     0, 0, 0, 0,  // biXPelsPerMeter
//!     0, 0, 0, 0,  // biYPelsPerMeter
//!     0, 0, 0, 0,  // biClrUsed
//!     0, 0, 0, 0,  // biClrImportant
//!     0x11, 0x22, 0x33, 0x00, // one pixel, stored blue-green-red, then pad
//! ];
//!
//! let header = DibHeader::parse(payload)?;
//! assert_eq!((header.width(), header.height()), (1, 1));
//!
//! let mut rgba = [0u8; 4];
//! assert_eq!(header.required_buffer_len(), rgba.len());
//! header.decode_into(payload, &mut rgba, AlphaMode::Straight)?;
//! assert_eq!(rgba, [0x33, 0x22, 0x11, 0xFF]);
//! # Ok::<(), rclip_core::Error>(())
//! ```
//!
//! # What this crate does not do
//!
//! PNG, JPEG and TIFF are deliberately out of scope, including the `BI_JPEG`
//! and `BI_PNG` compression values that can technically appear in a DIB header.
//! Those formats have good decoders already; these two do not. `BI_RLE4` and
//! `BI_RLE8` are likewise unimplemented — nothing in a modern clipboard writes
//! them — and are reported as [`ErrorKind::Unsupported`] rather than guessed
//! at.
//!
//! [`ErrorKind::Unsupported`]: rclip_core::ErrorKind::Unsupported
//!
//! # The alpha problem
//!
//! `CF_DIBV5` has no agreed alpha convention and no in-band signal for one.
//! Chromium and Firefox write premultiplied RGBA; XnView and Photoshop read the
//! same bytes as straight. Decoding therefore takes an explicit [`AlphaMode`]
//! from the caller instead of picking one quietly. See its documentation for
//! why [`AlphaMode::Guess`] is labelled a heuristic and not a detector.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

#[cfg(feature = "alloc")]
extern crate alloc;

// There is no `extern crate std` here even under the `std` feature: nothing in
// this crate needs it. The feature exists only to forward `rclip-core/std`, so
// that `rclip_core::Error` implements `std::error::Error` for callers who want
// `?` into an owned error type.

pub mod decode;
pub mod encode;
pub mod header;

pub use decode::AlphaMode;
pub use encode::{encode_v5_into, encoded_v5_len};
pub use header::{
    ChannelMask, DibHeader, HeaderVersion, Masks, BITMAPCOREHEADER_SIZE, BITMAPINFOHEADER_SIZE,
    BITMAPV2INFOHEADER_SIZE, BITMAPV3INFOHEADER_SIZE, BITMAPV4HEADER_SIZE, BITMAPV5HEADER_SIZE,
    BI_ALPHABITFIELDS, BI_BITFIELDS, BI_JPEG, BI_PNG, BI_RGB, BI_RLE4, BI_RLE8, LCS_CALIBRATED_RGB,
    LCS_GM_IMAGES, LCS_SRGB, LCS_WINDOWS_COLOR_SPACE, PROFILE_EMBEDDED, PROFILE_LINKED,
};

#[cfg(feature = "alloc")]
pub use encode::encode_v5;

use rclip_core::Result;

/// Parse and decode in one call, writing into a caller-provided buffer.
///
/// Returns the parsed header so the caller knows the dimensions it just
/// decoded. Size `dst` from [`DibHeader::required_buffer_len`] after a separate
/// [`DibHeader::parse`] if you need the size before committing to a buffer.
pub fn decode_into(src: &[u8], dst: &mut [u8], alpha: AlphaMode) -> Result<DibHeader> {
    let header = DibHeader::parse(src)?;
    header.decode_into(src, dst, alpha)?;
    Ok(header)
}

/// A decoded image: 8-bit RGBA, top row first, no row padding.
#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes of `R, G, B, A`.
    pub pixels: alloc::vec::Vec<u8>,
}

/// Parse and decode into a freshly allocated [`RgbaImage`].
///
/// The allocation is sized from the header, which is why
/// [`DibHeader::parse`] refuses anything over `rclip_core::MAX_PIXELS` before
/// this function ever sees it.
#[cfg(feature = "alloc")]
pub fn decode(src: &[u8], alpha: AlphaMode) -> Result<RgbaImage> {
    let header = DibHeader::parse(src)?;
    let mut pixels = alloc::vec![0u8; header.required_buffer_len()];
    header.decode_into(src, &mut pixels, alpha)?;
    Ok(RgbaImage {
        width: header.width(),
        height: header.height(),
        pixels,
    })
}

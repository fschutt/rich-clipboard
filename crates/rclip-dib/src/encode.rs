//! RGBA8 out to `CF_DIBV5` and to `CF_DIB`.
//!
//! Two output shapes, one per clipboard format, and each is the only shape its
//! format can carry honestly:
//!
//! - [`encode_v5_into`] writes a 124-byte `BITMAPV5HEADER`, 32 bpp,
//!   `BI_BITFIELDS`, with an explicit `bV5AlphaMask`. Writing `BI_RGB` at 32 bpp
//!   instead would be smaller and wrong — the high byte is undefined there, so a
//!   conformant reader is entitled to drop the alpha channel on the floor. The
//!   mask is what makes the transparency survive the trip.
//! - [`encode_dib_into`] writes a 40-byte `BITMAPINFOHEADER`, 24 bpp, `BI_RGB`.
//!   That header has no alpha channel and no colour-space field at all, so the
//!   image goes out opaque and in whatever space it was already in. See
//!   [`Flatten`] for the one decision that leaves the caller.
//!
//! Why 24 bpp and not 32 for `CF_DIB`: at 32 bpp under a `BITMAPINFOHEADER` the
//! fourth byte of every pixel is undefined, and readers disagree about it in
//! both directions — this crate's own decoder treats an all-non-zero fourth byte
//! as alpha, and plenty of others treat it as alpha unconditionally, which turns
//! a 0x00 filler into a fully transparent image. 24 bpp has no fourth byte to
//! disagree about, and it is what the older consumers `CF_DIB` exists for
//! actually expect.

use rclip_core::{Error, ErrorKind, Result};

use crate::decode::AlphaMode;
use crate::header::{
    BITMAPINFOHEADER_SIZE, BITMAPV5HEADER_SIZE, BI_BITFIELDS, BI_RGB, LCS_GM_IMAGES, LCS_SRGB,
};

/// Channel masks this encoder writes: the conventional Windows BGRA-in-a-DWORD
/// layout, which is also what every producer worth interoperating with emits.
const RED_MASK: u32 = 0x00FF_0000;
const GREEN_MASK: u32 = 0x0000_FF00;
const BLUE_MASK: u32 = 0x0000_00FF;
const ALPHA_MASK: u32 = 0xFF00_0000;

/// Exact size of the `CF_DIBV5` payload [`encode_v5_into`] produces.
///
/// At 32 bpp the row stride is `width * 4`, already a multiple of four, so
/// there is never any row padding to account for.
pub fn encoded_v5_len(width: u32, height: u32) -> Result<usize> {
    let pixels = validated_pixel_count(width, height)?;
    let body = pixels
        .checked_mul(4)
        .ok_or(Error::new(ErrorKind::TooLarge, 0))?;
    usize::try_from(u64::from(BITMAPV5HEADER_SIZE) + body)
        .map_err(|_| Error::new(ErrorKind::TooLarge, 0))
}

/// Encode `rgba` (top row first, `R, G, B, A`) as a packed `CF_DIBV5` payload.
///
/// Returns the number of bytes written, which is always
/// [`encoded_v5_len`]. `dst` may be longer; the excess is untouched.
///
/// `alpha` says what to put in the file, not what `rgba` contains: `rgba` is
/// always straight, and [`AlphaMode::Premultiplied`] asks for it to be
/// premultiplied on the way out (what Chromium and Firefox expect to read
/// back). [`AlphaMode::Guess`] is rejected — it is a policy for reading bytes
/// somebody else wrote, and there is nothing to guess about pixels you own.
pub fn encode_v5_into(
    width: u32,
    height: u32,
    rgba: &[u8],
    alpha: AlphaMode,
    dst: &mut [u8],
) -> Result<usize> {
    let premultiply = match alpha {
        AlphaMode::Straight => false,
        AlphaMode::Premultiplied => true,
        AlphaMode::Guess => return Err(Error::new(ErrorKind::Unsupported, 0)),
    };

    let total = encoded_v5_len(width, height)?;
    let row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or(Error::new(ErrorKind::TooLarge, 0))?;
    let need = row_bytes
        .checked_mul(height as usize)
        .ok_or(Error::new(ErrorKind::TooLarge, 0))?;
    if rgba.len() < need {
        return Err(Error::new(ErrorKind::BadLength, 0));
    }

    let mut w = Cursor::new(
        dst.get_mut(..total)
            .ok_or(Error::new(ErrorKind::BadLength, 0))?,
    );

    w.u32(BITMAPV5HEADER_SIZE)?;
    w.i32(i32::try_from(width).map_err(|_| Error::new(ErrorKind::TooLarge, 0))?)?;
    // Positive height: bottom-up, the format's default. Top-down is legal and
    // half the size of the code, but it is also the case more third-party
    // readers get wrong, and a clipboard payload only has to please strangers.
    w.i32(i32::try_from(height).map_err(|_| Error::new(ErrorKind::TooLarge, 0))?)?;
    w.u16(1)?; // bV5Planes: must be 1
    w.u16(32)?; // bV5BitCount
    w.u32(BI_BITFIELDS)?;
    w.u32(u32::try_from(need).map_err(|_| Error::new(ErrorKind::TooLarge, 0))?)?; // bV5SizeImage
    w.u32(0)?; // bV5XPelsPerMeter: unknown
    w.u32(0)?; // bV5YPelsPerMeter: unknown
    w.u32(0)?; // bV5ClrUsed: no palette at 32 bpp
    w.u32(0)?; // bV5ClrImportant
    w.u32(RED_MASK)?;
    w.u32(GREEN_MASK)?;
    w.u32(BLUE_MASK)?;
    w.u32(ALPHA_MASK)?;
    w.u32(LCS_SRGB)?; // bV5CSType
    w.zeros(36)?; // bV5Endpoints: ignored unless LCS_CALIBRATED_RGB
    w.u32(0)?; // bV5GammaRed
    w.u32(0)?; // bV5GammaGreen
    w.u32(0)?; // bV5GammaBlue
    w.u32(LCS_GM_IMAGES)?; // bV5Intent: perceptual
    w.u32(0)?; // bV5ProfileData
    w.u32(0)?; // bV5ProfileSize
    w.u32(0)?; // bV5Reserved
    debug_assert_eq!(
        w.pos(),
        BITMAPV5HEADER_SIZE as usize,
        "BITMAPV5HEADER is 124 bytes"
    );

    // Bottom-up: the last row of the image goes first on the wire.
    for src_y in (0..height as usize).rev() {
        let start = src_y * row_bytes;
        let row = rgba
            .get(start..start + row_bytes)
            .ok_or(Error::new(ErrorKind::BadLength, 0))?;
        for p in row.chunks_exact(4) {
            let a = p[3];
            let (r, g, b) = if premultiply {
                (premul(p[0], a), premul(p[1], a), premul(p[2], a))
            } else {
                (p[0], p[1], p[2])
            };
            // The masks above describe a little-endian DWORD, so on the wire
            // that is blue, green, red, alpha.
            w.bytes(&[b, g, r, a])?;
        }
    }

    Ok(w.pos())
}

/// [`encode_v5_into`] with the buffer allocated for you.
#[cfg(feature = "alloc")]
pub fn encode_v5(
    width: u32,
    height: u32,
    rgba: &[u8],
    alpha: AlphaMode,
) -> Result<alloc::vec::Vec<u8>> {
    // `encoded_v5_len` applies the MAX_PIXELS guard, so this capacity is
    // bounded before it is requested.
    let len = encoded_v5_len(width, height)?;
    let mut out = alloc::vec![0u8; len];
    let written = encode_v5_into(width, height, rgba, alpha, &mut out)?;
    debug_assert_eq!(written, len);
    Ok(out)
}

/// What an encoder that cannot store alpha does with it.
///
/// `CF_DIB` is a `BITMAPINFOHEADER`, which has no alpha channel — there is no
/// mask, no fourth channel and nowhere to record that a pixel was ever partly
/// transparent. Something has to happen to the alpha the caller holds, and the
/// two things that can happen produce visibly different images, so the caller
/// picks rather than the crate.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Flatten {
    /// Drop the alpha channel and write the colour channels unchanged.
    ///
    /// Correct for straight-alpha input whose colour is meaningful independent
    /// of its coverage — an anti-aliased glyph keeps its glyph colour rather
    /// than fading toward a background it was never composited against. It is
    /// also what Windows itself does when it converts a `CF_DIBV5` to `CF_DIB`
    /// for a legacy consumer.
    Discard,
    /// Composite over an opaque background colour, `R, G, B`.
    ///
    /// Treats the input as *straight* alpha: `out = c * a + bg * (1 - a)`. This
    /// is what a paint program does on paste, and what makes a screenshot with a
    /// rounded transparent corner look right rather than showing the raw colour
    /// underneath. [`Flatten::OVER_WHITE`] is the usual choice.
    Over([u8; 3]),
}

impl Flatten {
    /// Composite over opaque white, which is what most document editors paste
    /// onto.
    pub const OVER_WHITE: Self = Self::Over([0xFF, 0xFF, 0xFF]);

    /// Composite over opaque black.
    pub const OVER_BLACK: Self = Self::Over([0x00, 0x00, 0x00]);
}

/// Exact size of the `CF_DIB` payload [`encode_dib_into`] produces.
///
/// Unlike the 32-bpp V5 form, a 24-bpp row is *not* automatically a multiple of
/// four bytes: a 3-pixel row is 9 bytes of colour and 12 bytes on the wire. That
/// pad is the classic DIB off-by-one, and it is in this number.
pub fn encoded_dib_len(width: u32, height: u32) -> Result<usize> {
    validated_pixel_count(width, height)?;
    let stride = dib_stride(width);
    let total = u64::from(BITMAPINFOHEADER_SIZE) + stride * u64::from(height);
    usize::try_from(total).map_err(|_| Error::new(ErrorKind::TooLarge, 0))
}

/// Encode `rgba` (top row first, `R, G, B, A`) as a packed `CF_DIB` payload:
/// a 40-byte `BITMAPINFOHEADER`, 24 bpp, `BI_RGB`, bottom-up.
///
/// Returns the number of bytes written, which is always [`encoded_dib_len`].
/// `dst` may be longer; the excess is untouched.
///
/// `flatten` says what becomes of the alpha channel, which this format cannot
/// carry. There is no `AlphaMode` here for the same reason: premultiplied
/// storage is a property of a format with an alpha channel, and this one has
/// none.
///
/// # Errors
///
/// [`ErrorKind::Malformed`] for a zero dimension, [`ErrorKind::TooLarge`] past
/// `rclip_core::MAX_PIXELS`, and [`ErrorKind::BadLength`] if `rgba` is shorter
/// than `width * height * 4` or `dst` shorter than [`encoded_dib_len`].
pub fn encode_dib_into(
    width: u32,
    height: u32,
    rgba: &[u8],
    flatten: Flatten,
    dst: &mut [u8],
) -> Result<usize> {
    let total = encoded_dib_len(width, height)?;
    let src_row = (width as usize)
        .checked_mul(4)
        .ok_or(Error::new(ErrorKind::TooLarge, 0))?;
    let need = src_row
        .checked_mul(height as usize)
        .ok_or(Error::new(ErrorKind::TooLarge, 0))?;
    if rgba.len() < need {
        return Err(Error::new(ErrorKind::BadLength, 0));
    }
    // Exact: `encoded_dib_len` already proved the product fits a usize.
    let stride = dib_stride(width) as usize;
    let pad = stride - (width as usize) * 3;

    let mut w = Cursor::new(
        dst.get_mut(..total)
            .ok_or(Error::new(ErrorKind::BadLength, 0))?,
    );

    w.u32(BITMAPINFOHEADER_SIZE)?;
    w.i32(i32::try_from(width).map_err(|_| Error::new(ErrorKind::TooLarge, 0))?)?;
    // Positive height: bottom-up, the format's default and the one every
    // ancient consumer of CF_DIB was written against.
    w.i32(i32::try_from(height).map_err(|_| Error::new(ErrorKind::TooLarge, 0))?)?;
    w.u16(1)?; // biPlanes: must be 1
    w.u16(24)?; // biBitCount
    w.u32(BI_RGB)?;
    // biSizeImage may be zero for BI_RGB, but a real byte count is what a
    // reader that wants to find the end of the payload uses, and writing it
    // costs nothing.
    w.u32(
        u32::try_from(total - BITMAPINFOHEADER_SIZE as usize)
            .map_err(|_| Error::new(ErrorKind::TooLarge, 0))?,
    )?;
    w.u32(0)?; // biXPelsPerMeter: unknown
    w.u32(0)?; // biYPelsPerMeter: unknown
    w.u32(0)?; // biClrUsed: no palette above 8 bpp
    w.u32(0)?; // biClrImportant
    debug_assert_eq!(
        w.pos(),
        BITMAPINFOHEADER_SIZE as usize,
        "BITMAPINFOHEADER is 40 bytes"
    );

    // Bottom-up: the last row of the image goes first on the wire.
    for src_y in (0..height as usize).rev() {
        let start = src_y * src_row;
        let row = rgba
            .get(start..start + src_row)
            .ok_or(Error::new(ErrorKind::BadLength, 0))?;
        for p in row.chunks_exact(4) {
            let (r, g, b) = match flatten {
                Flatten::Discard => (p[0], p[1], p[2]),
                Flatten::Over(bg) => (
                    over(p[0], p[3], bg[0]),
                    over(p[1], p[3], bg[1]),
                    over(p[2], p[3], bg[2]),
                ),
            };
            // 24-bpp DIB pixels are stored blue, green, red.
            w.bytes(&[b, g, r])?;
        }
        // The row pad is written, not skipped: `dst` is the caller's buffer and
        // may hold anything, and leaving three bytes of it inside a clipboard
        // payload would publish whatever they were.
        w.zeros(pad)?;
    }

    Ok(w.pos())
}

/// [`encode_dib_into`] with the buffer allocated for you.
///
/// # Errors
///
/// As [`encode_dib_into`].
#[cfg(feature = "alloc")]
pub fn encode_dib(
    width: u32,
    height: u32,
    rgba: &[u8],
    flatten: Flatten,
) -> Result<alloc::vec::Vec<u8>> {
    let len = encoded_dib_len(width, height)?;
    let mut out = alloc::vec![0u8; len];
    let written = encode_dib_into(width, height, rgba, flatten, &mut out)?;
    debug_assert_eq!(written, len);
    Ok(out)
}

/// Bytes per 24-bpp row including the pad to a 4-byte boundary.
///
/// `u64` throughout: `width` is a `u32`, so `width * 3` overflows a `u32` at
/// about 1.4 billion pixels — which `validated_pixel_count` has already ruled
/// out, but the arithmetic should not be the thing relying on that.
const fn dib_stride(width: u32) -> u64 {
    (width as u64 * 24).div_ceil(32) * 4
}

/// Composite one straight-alpha channel over an opaque background.
const fn over(c: u8, a: u8, bg: u8) -> u8 {
    let a = a as u32;
    // Round to nearest: a == 255 must land exactly on `c`, and a == 0 exactly
    // on `bg`, or a fully opaque image would drift by a level.
    ((c as u32 * a + bg as u32 * (255 - a) + 127) / 255) as u8
}

fn validated_pixel_count(width: u32, height: u32) -> Result<u64> {
    if width == 0 || height == 0 {
        return Err(Error::new(ErrorKind::Malformed, 0));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > rclip_core::MAX_PIXELS {
        return Err(Error::new(ErrorKind::TooLarge, 0));
    }
    Ok(pixels)
}

const fn premul(c: u8, a: u8) -> u8 {
    // Round to nearest so that a round-trip through unpremultiply is stable for
    // the common alphas instead of drifting one level darker each pass.
    ((c as u32 * a as u32 + 127) / 255) as u8
}

/// Minimal forward writer. The decoder side has `rclip_core::Reader`; there is
/// no shared counterpart, and one that only needs four methods is not worth a
/// dependency.
struct Cursor<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    const fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    const fn pos(&self) -> usize {
        self.pos
    }

    fn bytes(&mut self, b: &[u8]) -> Result<()> {
        let end = self
            .pos
            .checked_add(b.len())
            .ok_or(Error::new(ErrorKind::TooLarge, self.pos))?;
        let slot = self
            .buf
            .get_mut(self.pos..end)
            .ok_or(Error::new(ErrorKind::BadLength, self.pos))?;
        slot.copy_from_slice(b);
        self.pos = end;
        Ok(())
    }

    fn zeros(&mut self, n: usize) -> Result<()> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(Error::new(ErrorKind::TooLarge, self.pos))?;
        let slot = self
            .buf
            .get_mut(self.pos..end)
            .ok_or(Error::new(ErrorKind::BadLength, self.pos))?;
        slot.fill(0);
        self.pos = end;
        Ok(())
    }

    fn u16(&mut self, v: u16) -> Result<()> {
        self.bytes(&v.to_le_bytes())
    }

    fn u32(&mut self, v: u32) -> Result<()> {
        self.bytes(&v.to_le_bytes())
    }

    fn i32(&mut self, v: i32) -> Result<()> {
        self.bytes(&v.to_le_bytes())
    }
}

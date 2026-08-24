//! RGBA8 out to `CF_DIBV5`.
//!
//! Only one output shape is offered: a 124-byte `BITMAPV5HEADER`, 32 bpp,
//! `BI_BITFIELDS`, with an explicit `bV5AlphaMask`. Writing `BI_RGB` at 32 bpp
//! instead would be smaller and wrong — the high byte is undefined there, so a
//! conformant reader is entitled to drop the alpha channel on the floor. The
//! mask is what makes the transparency survive the trip.

use rclip_core::{Error, ErrorKind, Result};

use crate::decode::AlphaMode;
use crate::header::{BITMAPV5HEADER_SIZE, BI_BITFIELDS, LCS_GM_IMAGES, LCS_SRGB};

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

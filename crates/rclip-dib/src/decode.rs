//! Pixel decoding: packed DIB in, RGBA8 out.
//!
//! Everything sized by a wire field was already checked in
//! [`DibHeader::parse`], so this module walks rows and never re-derives an
//! offset from the input.

use rclip_core::{Error, ErrorKind, Reader, Result};

use crate::header::{ChannelMask, DibHeader};

/// How the caller wants alpha interpreted.
///
/// `CF_DIBV5` carries no signal for this and the ecosystem is genuinely split:
/// Chromium and Firefox put *premultiplied* RGBA on the clipboard, while XnView
/// and Photoshop read the same bytes as *straight*. There is no bit in the
/// header that distinguishes them, so this crate refuses to guess silently and
/// makes it the caller's decision.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum AlphaMode {
    /// Colour channels are independent of alpha. Emit them unchanged.
    Straight,
    /// Colour channels were multiplied by alpha at write time; divide it back
    /// out. A pixel with `alpha == 0` decodes to transparent black, which is
    /// the only colour premultiplication can represent there.
    Premultiplied,
    /// Inspect the pixels and pick.
    ///
    /// **This is a heuristic, not a detection.** The only sound inference
    /// available is one-directional: a pixel whose red, green or blue exceeds
    /// its alpha cannot have been produced by premultiplication, so an image
    /// containing one is straight. The converse does not hold — a dark
    /// straight-alpha image satisfies `c <= a` everywhere and will be
    /// classified as premultiplied and wrongly brightened. Prefer an explicit
    /// mode whenever the source application is known.
    Guess,
}

impl DibHeader {
    /// Decode into a caller-provided RGBA8 buffer.
    ///
    /// `src` must be the same payload [`DibHeader::parse`] was given — every
    /// offset in `self` was validated against that buffer and no other. Passing
    /// a different one is rejected rather than trusted.
    ///
    /// `dst` must be at least [`DibHeader::required_buffer_len`] bytes. Exactly
    /// that many are written, top row first, as `R, G, B, A` quadruples.
    pub fn decode_into(&self, src: &[u8], dst: &mut [u8], alpha: AlphaMode) -> Result<()> {
        let need = self.required_buffer_len();
        let dst = dst
            .get_mut(..need)
            .ok_or(Error::new(ErrorKind::BadLength, 0))?;

        let r = Reader::new(src);
        // Cheap identity check. It cannot prove `src` is the buffer that was
        // parsed, but it catches the common mistake of pairing a header with
        // the wrong payload, which would otherwise decode as noise.
        if r.peek_u32_le_at(0)? != self.version().size() {
            return Err(Error::new(ErrorKind::BadMagic, 0));
        }

        // `pixel_offset - palette_offset` is the palette's byte length by
        // construction, which avoids recomputing `entries * 4` and its overflow
        // case.
        let palette_bytes = self.pixel_offset().saturating_sub(self.palette_offset());
        let palette = r.slice_at(self.palette_offset(), palette_bytes)?;
        let data = r.slice_at(self.pixel_offset(), self.image_bytes())?;

        let alpha_mask = self.effective_alpha_mask(data);

        let height = self.height() as usize;
        // Exact: `required_buffer_len` is `width * height * 4` and height > 0.
        let row_bytes = need / height;

        for (src_y, row) in data.chunks_exact(self.stride()).enumerate().take(height) {
            // Positive biHeight means the first row on the wire is the *last*
            // row of the image. Flipping this is the difference between a
            // picture and an upside-down picture, and nothing else complains.
            let dst_y = if self.is_top_down() {
                src_y
            } else {
                height - 1 - src_y
            };
            let start = dst_y
                .checked_mul(row_bytes)
                .ok_or(Error::new(ErrorKind::TooLarge, 0))?;
            let end = start
                .checked_add(row_bytes)
                .ok_or(Error::new(ErrorKind::TooLarge, 0))?;
            let out = dst
                .get_mut(start..end)
                .ok_or(Error::new(ErrorKind::BadLength, 0))?;
            self.decode_row(row, palette, out, alpha_mask)?;
        }

        if alpha_mask.is_present() {
            apply_alpha_policy(dst, alpha);
        }
        Ok(())
    }

    /// Decide whether the fourth byte of a 32-bpp pixel is really alpha.
    ///
    /// A 32-bpp `BITMAPINFOHEADER` has no alpha channel at all — the docs say
    /// the high byte of each `DWORD` "is not used", so its contents are
    /// undefined. Producers leave it zero constantly. Honouring it blindly
    /// turns every `CF_DIB` screenshot into a fully transparent image; ignoring
    /// it throws away the alpha that some producers do write there. The rule
    /// the plan settles on: treat it as alpha only when *every* pixel has a
    /// non-zero value, which no all-zeroes filler can satisfy.
    ///
    /// The same test covers a V4/V5 header that leaves `bV5AlphaMask` at zero.
    fn effective_alpha_mask(&self, data: &[u8]) -> ChannelMask {
        let declared = self.masks().alpha;
        if declared.is_present() {
            return declared;
        }
        if self.bit_count() != 32 {
            return ChannelMask::NONE;
        }
        const TOP_BYTE: u32 = 0xFF00_0000;
        // Only probe bits no colour channel claims. Under BI_BITFIELDS a mask
        // set may legitimately use all 32 bits for colour.
        if self.masks().uncovered() & TOP_BYTE != TOP_BYTE {
            return ChannelMask::NONE;
        }
        // At 32 bpp the stride is exactly `width * 4`, so there is no row
        // padding for this scan to mistake for a pixel.
        if data.chunks_exact(4).all(|p| p[3] != 0) {
            ChannelMask::new(TOP_BYTE)
        } else {
            ChannelMask::NONE
        }
    }

    fn decode_row(
        &self,
        row: &[u8],
        palette: &[u8],
        out: &mut [u8],
        alpha: ChannelMask,
    ) -> Result<()> {
        let m = self.masks();
        // `out` is exactly `width` quadruples and every `row` is at least
        // `width` pixels wide, so zipping stops on `out` and the stride padding
        // at the end of `row` is never read.
        match self.bit_count() {
            32 => {
                for (o, p) in out.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                    let px = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                    o[0] = m.red.extract(px);
                    o[1] = m.green.extract(px);
                    o[2] = m.blue.extract(px);
                    o[3] = if alpha.is_present() {
                        alpha.extract(px)
                    } else {
                        0xFF
                    };
                }
            }
            24 => {
                // No masks at 24 bpp: the spec fixes the byte order as blue,
                // green, red.
                for (o, p) in out.chunks_exact_mut(4).zip(row.chunks_exact(3)) {
                    o[0] = p[2];
                    o[1] = p[1];
                    o[2] = p[0];
                    o[3] = 0xFF;
                }
            }
            16 => {
                for (o, p) in out.chunks_exact_mut(4).zip(row.chunks_exact(2)) {
                    let px = u32::from(u16::from_le_bytes([p[0], p[1]]));
                    o[0] = m.red.extract(px);
                    o[1] = m.green.extract(px);
                    o[2] = m.blue.extract(px);
                    o[3] = if alpha.is_present() {
                        alpha.extract(px)
                    } else {
                        0xFF
                    };
                }
            }
            8 => {
                for (o, p) in out.chunks_exact_mut(4).zip(row.iter()) {
                    o.copy_from_slice(&self.palette_entry(palette, usize::from(*p))?);
                }
            }
            4 => {
                // Two pixels per byte, high nibble first: 0x1F is index 1 then
                // index 15, not the other way round.
                for (x, o) in out.chunks_exact_mut(4).enumerate() {
                    let byte = *row
                        .get(x / 2)
                        .ok_or(Error::new(ErrorKind::UnexpectedEof, self.pixel_offset()))?;
                    let idx = if x % 2 == 0 { byte >> 4 } else { byte & 0x0F };
                    o.copy_from_slice(&self.palette_entry(palette, usize::from(idx))?);
                }
            }
            1 => {
                // Eight pixels per byte, most significant bit leftmost.
                for (x, o) in out.chunks_exact_mut(4).enumerate() {
                    let byte = *row
                        .get(x / 8)
                        .ok_or(Error::new(ErrorKind::UnexpectedEof, self.pixel_offset()))?;
                    let idx = (byte >> (7 - (x % 8))) & 1;
                    o.copy_from_slice(&self.palette_entry(palette, usize::from(idx))?);
                }
            }
            // `parse` rejects every other bit count, so this is unreachable for
            // a `DibHeader` obtained the only way there is to obtain one.
            _ => return Err(Error::new(ErrorKind::Unsupported, 14)),
        }
        Ok(())
    }

    /// Look up one palette entry, remembering that `RGBQUAD` is stored blue
    /// first and that `rgbReserved` is documented as "must be zero" — it is not
    /// an alpha channel, so palettised images always decode opaque.
    fn palette_entry(&self, palette: &[u8], index: usize) -> Result<[u8; 4]> {
        let at = index
            .checked_mul(4)
            .ok_or(Error::new(ErrorKind::Malformed, self.palette_offset()))?;
        let end = at
            .checked_add(4)
            .ok_or(Error::new(ErrorKind::Malformed, self.palette_offset()))?;
        // An index past the end of a short palette is a real malformation, not
        // something to clamp: GDI would read whatever memory followed.
        let e = palette.get(at..end).ok_or(Error::new(
            ErrorKind::Malformed,
            self.palette_offset().saturating_add(at),
        ))?;
        Ok([e[2], e[1], e[0], 0xFF])
    }
}

/// Turn the decoded, as-stored RGBA into straight RGBA per the caller's policy.
fn apply_alpha_policy(dst: &mut [u8], mode: AlphaMode) {
    let premultiplied = match mode {
        AlphaMode::Straight => false,
        AlphaMode::Premultiplied => true,
        AlphaMode::Guess => !looks_straight(dst),
    };
    if premultiplied {
        unpremultiply(dst);
    }
}

/// The one-directional half of the alpha heuristic: `c > a` is unreachable if
/// `c` was multiplied by `a / 255`.
fn looks_straight(rgba: &[u8]) -> bool {
    rgba.chunks_exact(4)
        .any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3])
}

fn unpremultiply(rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3];
        if a == 0xFF {
            continue;
        }
        if a == 0 {
            // Premultiplication maps every colour to zero here; there is no
            // information left to recover, and dividing would be by zero.
            p[0] = 0;
            p[1] = 0;
            p[2] = 0;
            continue;
        }
        let a32 = u32::from(a);
        for c in p.iter_mut().take(3) {
            *c = ((u32::from(*c) * 255 + a32 / 2) / a32).min(255) as u8;
        }
    }
}

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

        if self.is_rle() {
            // A run-length stream shares nothing with the packed path: no
            // stride, no row chunking, no alpha. It also cannot reach the
            // `chunks_exact(self.stride())` below, which would panic on the
            // zero stride an RLE header reports.
            return self.decode_rle(palette, data, dst);
        }

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

    /// Decode a [`BI_RLE4`] or [`BI_RLE8`] run-length stream into `dst`.
    ///
    /// The grammar is the same for both depths and is four cases wide, three of
    /// which are escapes:
    ///
    /// ```text
    /// n>0  b    encoded run: n pixels. RLE8 repeats byte b; RLE4 alternates
    ///           its high and low nibble, high first.
    /// 0    0    end of line
    /// 0    1    end of bitmap
    /// 0    2    delta: two more bytes, an unsigned dx and dy
    /// 0    n>2  absolute run: n literal indices follow, padded to a WORD
    /// ```
    ///
    /// Every one of those is somewhere a hostile stream tries to leave the
    /// bitmap, so each is bounded explicitly:
    ///
    /// - **Termination.** Each iteration consumes at least the two bytes read
    ///   at the top of the loop and [`Reader`] only moves forward, so the loop
    ///   runs at most `data.len() / 2` times whatever the bytes say. No escape
    ///   can rewind the cursor, because there is no cursor to rewind — the
    ///   delta moves the *pixel* position, not the read position.
    /// - **Runs.** A run is checked against the row and the bitmap *before* any
    ///   of it is written, so a count that would spill into the next row, or
    ///   into no row at all, fails cleanly instead of half-drawing.
    /// - **Absolute runs.** The literal bytes come through [`Reader::take`], so
    ///   a run claiming more bytes than remain is [`ErrorKind::UnexpectedEof`]
    ///   rather than a read past the end.
    /// - **Deltas.** `dx` and `dy` are unsigned, so the pixel position only ever
    ///   moves forward *in stream order* — but the rows are stored bottom-up, so
    ///   advancing `y` walks backwards through `dst`. Both edges are therefore
    ///   checked, and the addition itself is checked, since `usize::MAX` is two
    ///   deltas away on a 16-bit-ish budget only in theory but free to guard.
    ///
    /// Pixels no run covers keep the zero `dst` was filled with — transparent
    /// black. RLE is the one DIB encoding where "not covered" is expressible: a
    /// delta or an early end-of-bitmap leaves a hole, and GDI fills it with
    /// whatever the destination already held. There is no such thing here, so a
    /// hole is reported as a hole rather than as palette entry zero, which is a
    /// real colour a real run could have written.
    ///
    /// A stream that simply runs out without an end-of-bitmap marker is
    /// accepted: `biSizeImage` regularly excludes the terminator, and the rows
    /// decoded so far are not made wrong by its absence.
    ///
    /// [`BI_RLE4`]: crate::BI_RLE4
    /// [`BI_RLE8`]: crate::BI_RLE8
    /// [`Reader`]: rclip_core::Reader
    /// [`Reader::take`]: rclip_core::Reader::take
    fn decode_rle(&self, palette: &[u8], data: &[u8], dst: &mut [u8]) -> Result<()> {
        dst.fill(0);

        let width = self.width() as usize;
        let height = self.height() as usize;
        let four_bit = self.bit_count() == 4;
        // Offsets in errors are into the whole payload, not into the run
        // stream, so that they line up with a hex dump the way every other
        // error in this workspace does.
        let base = self.pixel_offset();
        let rebase = |e: Error| Error::new(e.kind, base.saturating_add(e.offset));

        let mut r = Reader::new(data);
        let mut x = 0usize;
        let mut y = 0usize;

        while r.remaining_len() >= 2 {
            let at = base.saturating_add(r.pos());
            let count = r.u8().map_err(rebase)?;
            let second = r.u8().map_err(rebase)?;

            if count != 0 {
                let n = usize::from(count);
                check_run(x, y, n, width, height, at)?;
                for i in 0..n {
                    let index = if four_bit {
                        // High nibble first: a run of 3 from 0x1F is 1, 15, 1.
                        if i % 2 == 0 {
                            second >> 4
                        } else {
                            second & 0x0F
                        }
                    } else {
                        second
                    };
                    self.rle_put(palette, dst, x + i, y, index, at)?;
                }
                x += n;
                continue;
            }

            match second {
                // End of line. `y` may now be `height`; that is only wrong if
                // something goes on to write a pixel, which `check_run` catches.
                0 => {
                    x = 0;
                    y += 1;
                }
                // End of bitmap. Whatever follows is padding.
                1 => return Ok(()),
                2 => {
                    let dx = usize::from(r.u8().map_err(rebase)?);
                    let dy = usize::from(r.u8().map_err(rebase)?);
                    x = x
                        .checked_add(dx)
                        .ok_or(Error::new(ErrorKind::TooLarge, at))?;
                    y = y
                        .checked_add(dy)
                        .ok_or(Error::new(ErrorKind::TooLarge, at))?;
                    // `== width` / `== height` are the legal "just past the
                    // last pixel" positions an encoder reaches at the end of a
                    // row or of the image; anything beyond names a pixel that
                    // does not exist.
                    if x > width || y > height {
                        return Err(Error::new(ErrorKind::Malformed, at));
                    }
                }
                n => {
                    let n = usize::from(n);
                    check_run(x, y, n, width, height, at)?;
                    let bytes = if four_bit { n.div_ceil(2) } else { n };
                    let run = r.take(bytes).map_err(rebase)?;
                    for i in 0..n {
                        // `run` is exactly `bytes` long and `i / 2 < bytes` by
                        // construction, but `n` came off the wire, so the index
                        // goes through `get` rather than `[]` all the same.
                        let index = if four_bit {
                            let b = *run
                                .get(i / 2)
                                .ok_or(Error::new(ErrorKind::UnexpectedEof, at))?;
                            if i % 2 == 0 {
                                b >> 4
                            } else {
                                b & 0x0F
                            }
                        } else {
                            *run.get(i).ok_or(Error::new(ErrorKind::UnexpectedEof, at))?
                        };
                        self.rle_put(palette, dst, x + i, y, index, at)?;
                    }
                    x += n;
                    // Absolute runs are padded to a WORD boundary. A stream
                    // that omits the final pad byte is tolerated — the next
                    // iteration ends the loop anyway — but one that has it must
                    // not leave it to be read as a count.
                    if bytes % 2 == 1 && r.remaining_len() >= 1 {
                        r.skip(1).map_err(rebase)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Write one palettised pixel of an RLE stream at `(x, y)`, where `y`
    /// counts rows from the bottom.
    fn rle_put(
        &self,
        palette: &[u8],
        dst: &mut [u8],
        x: usize,
        y: usize,
        index: u8,
        at: usize,
    ) -> Result<()> {
        // Bottom-up, always: `parse` rejects a top-down RLE header, so row `y`
        // of the stream is row `height - 1 - y` of the image.
        let dst_y = (self.height() as usize)
            .checked_sub(y.saturating_add(1))
            .ok_or(Error::new(ErrorKind::Malformed, at))?;
        let start = dst_y
            .checked_mul(self.width() as usize)
            .and_then(|v| v.checked_add(x))
            .and_then(|v| v.checked_mul(4))
            .ok_or(Error::new(ErrorKind::TooLarge, at))?;
        let end = start
            .checked_add(4)
            .ok_or(Error::new(ErrorKind::TooLarge, at))?;
        let px = self.palette_entry(palette, usize::from(index))?;
        dst.get_mut(start..end)
            .ok_or(Error::new(ErrorKind::Malformed, at))?
            .copy_from_slice(&px);
        Ok(())
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
        //
        // The *offset* is clamped, though, and only the offset. `index` came
        // off the wire, so `palette_offset + index * 4` can name a byte the
        // buffer does not have — and `Error::offset` is documented as a byte
        // offset *into the buffer the parser was handed*, which an out-of-range
        // number is not. `pixel_offset` is the first byte past the palette and
        // is known to be in bounds, so it is where "the lookup ran off the end
        // of the palette" points.
        let e = palette.get(at..end).ok_or(Error::new(
            ErrorKind::Malformed,
            self.palette_offset()
                .saturating_add(at)
                .min(self.pixel_offset()),
        ))?;
        Ok([e[2], e[1], e[0], 0xFF])
    }
}

/// Reject a run before any of it is written.
///
/// Both halves matter. `y >= height` is a stream that kept going after the top
/// of the image — the "more rows than the bitmap has" case, which an end-of-line
/// per row plus one more run produces. `x + n > width` is a run that would spill
/// into the row above it, which in a bottom-up bitmap is a write to pixels the
/// encoder never named.
fn check_run(x: usize, y: usize, n: usize, width: usize, height: usize, at: usize) -> Result<()> {
    if y >= height {
        return Err(Error::new(ErrorKind::Malformed, at));
    }
    let end = x
        .checked_add(n)
        .ok_or(Error::new(ErrorKind::TooLarge, at))?;
    if end > width {
        return Err(Error::new(ErrorKind::Malformed, at));
    }
    Ok(())
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

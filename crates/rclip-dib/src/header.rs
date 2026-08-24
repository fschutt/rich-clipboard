//! The DIB information header, in all five sizes that turn up on a clipboard.
//!
//! A packed DIB has no `BITMAPFILEHEADER`: the payload *starts* at the
//! information header, and the first `u32` — the header size — is the only
//! discriminator there is. Everything else about the layout (where the bitfield
//! masks live, whether there is a palette, where the pixels begin) hangs off
//! that one number, so it is parsed and validated before anything else.

use rclip_core::{Error, ErrorKind, Reader, Result};

/// `BITMAPCOREHEADER` (OS/2 1.x, Windows 2.0). Rejected — see [`HeaderVersion`].
pub const BITMAPCOREHEADER_SIZE: u32 = 12;
/// `BITMAPINFOHEADER` — what `CF_DIB` carries.
pub const BITMAPINFOHEADER_SIZE: u32 = 40;
/// Undocumented `BITMAPV2INFOHEADER`: `BITMAPINFOHEADER` + 3 mask DWORDs.
pub const BITMAPV2INFOHEADER_SIZE: u32 = 52;
/// Undocumented `BITMAPV3INFOHEADER`: `BITMAPV2INFOHEADER` + an alpha mask.
pub const BITMAPV3INFOHEADER_SIZE: u32 = 56;
/// `BITMAPV4HEADER`.
pub const BITMAPV4HEADER_SIZE: u32 = 108;
/// `BITMAPV5HEADER` — what `CF_DIBV5` carries.
pub const BITMAPV5HEADER_SIZE: u32 = 124;

/// `biCompression`: uncompressed, channel layout implied by `biBitCount`.
pub const BI_RGB: u32 = 0;
/// 8-bpp run-length encoding. Valid only with `biBitCount == 8`.
pub const BI_RLE8: u32 = 1;
/// 4-bpp run-length encoding. Valid only with `biBitCount == 4`.
pub const BI_RLE4: u32 = 2;
/// Uncompressed, channel layout given by explicit DWORD masks.
pub const BI_BITFIELDS: u32 = 3;
/// An embedded JPEG stream. Not implemented — out of scope for this crate.
pub const BI_JPEG: u32 = 4;
/// An embedded PNG stream. Not implemented — out of scope for this crate.
pub const BI_PNG: u32 = 5;
/// Windows CE extension: like [`BI_BITFIELDS`] but with a fourth (alpha) mask.
pub const BI_ALPHABITFIELDS: u32 = 6;

/// `bV5CSType` = `LCS_CALIBRATED_RGB`: endpoints and gamma are meaningful.
pub const LCS_CALIBRATED_RGB: u32 = 0x0000_0000;
/// `bV5CSType` = `'sRGB'`. The FourCC is a `DWORD`, so the bytes on the wire
/// read `B`, `G`, `R`, `s` — little-endian, like everything else here.
pub const LCS_SRGB: u32 = 0x7352_4742;
/// `bV5CSType` = `'Win '`, the system default colour space (also sRGB).
pub const LCS_WINDOWS_COLOR_SPACE: u32 = 0x5769_6E20;
/// `bV5CSType` = `'LINK'`: `bV5ProfileData` is a file name.
pub const PROFILE_LINKED: u32 = 0x4C49_4E4B;
/// `bV5CSType` = `'MBED'`: `bV5ProfileData` is an embedded ICC profile.
pub const PROFILE_EMBEDDED: u32 = 0x4D42_4544;

/// `bV5Intent` = perceptual. What an image (as opposed to a chart) wants.
pub const LCS_GM_IMAGES: u32 = 4;

/// One CIE 1931 XYZ colour from a `CIEXYZTRIPLE`, in `FXPT2DOT30` fixed point.
///
/// `FXPT2DOT30` is a signed 32-bit fixed-point number with two integer bits and
/// thirty fraction bits, so the raw value divided by `2^30` is the number the
/// producer meant. The fields are kept raw because that is what was on the
/// wire; [`CieXyz::to_f32`] does the division.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct CieXyz {
    /// `ciexyzX`.
    pub x: i32,
    /// `ciexyzY`.
    pub y: i32,
    /// `ciexyzZ`.
    pub z: i32,
}

impl CieXyz {
    /// The three components as ordinary numbers, `raw / 2^30`.
    #[must_use]
    pub fn to_f32(self) -> (f32, f32, f32) {
        (fxpt2dot30(self.x), fxpt2dot30(self.y), fxpt2dot30(self.z))
    }
}

/// `bV4Endpoints` / `bV5Endpoints`: the `CIEXYZTRIPLE` at bytes 60..96.
///
/// Reported, never applied. See [`DibHeader::endpoints`] for why.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct Endpoints {
    /// `ciexyzRed`.
    pub red: CieXyz,
    /// `ciexyzGreen`.
    pub green: CieXyz,
    /// `ciexyzBlue`.
    pub blue: CieXyz,
}

/// `bV4GammaRed`/`Green`/`Blue`: the three `DWORD`s at bytes 96..108.
///
/// Each is unsigned 16.16 fixed point, so the raw value divided by `65536` is
/// the gamma the producer meant. A typical `2.2` arrives as `0x0002_3333`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
pub struct Gamma {
    /// `bV4GammaRed`, raw 16.16.
    pub red: u32,
    /// `bV4GammaGreen`, raw 16.16.
    pub green: u32,
    /// `bV4GammaBlue`, raw 16.16.
    pub blue: u32,
}

impl Gamma {
    /// The three curves as ordinary numbers, `raw / 65536`.
    #[must_use]
    pub fn to_f32(self) -> (f32, f32, f32) {
        (
            fixed16_16(self.red),
            fixed16_16(self.green),
            fixed16_16(self.blue),
        )
    }
}

/// `FXPT2DOT30` — two integer bits, thirty fraction bits.
fn fxpt2dot30(raw: i32) -> f32 {
    raw as f32 / (1u32 << 30) as f32
}

/// Unsigned 16.16 fixed point, as `bV4GammaRed` and friends use.
fn fixed16_16(raw: u32) -> f32 {
    raw as f32 / 65536.0
}

/// Which information header the payload actually carries.
///
/// The two undocumented Adobe variants are accepted because they exist in the
/// wild — Photoshop and older GIMP builds write 52- and 56-byte headers — and
/// treating them as "unknown size" would reject perfectly decodable images.
/// There is deliberately no variant for the 12-byte `BITMAPCOREHEADER`: it uses
/// 16-bit dimensions and a 3-byte `RGBTRIPLE` palette, an entirely different
/// layout that no clipboard on any supported Windows version produces.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum HeaderVersion {
    /// 40 bytes. `CF_DIB`.
    Info,
    /// 52 bytes. Undocumented; RGB masks inside the header.
    V2,
    /// 56 bytes. Undocumented; RGB + alpha masks inside the header.
    V3,
    /// 108 bytes. `BITMAPV4HEADER`.
    V4,
    /// 124 bytes. `BITMAPV5HEADER`. `CF_DIBV5`.
    V5,
}

impl HeaderVersion {
    /// Map `biSize` onto a version.
    ///
    /// Any other size is refused rather than rounded down to the nearest known
    /// header: a wrong guess here does not fail loudly, it shifts the pixel
    /// data and produces a skewed image that looks like a decoder bug.
    pub fn from_size(size: u32) -> Result<Self> {
        match size {
            BITMAPINFOHEADER_SIZE => Ok(Self::Info),
            BITMAPV2INFOHEADER_SIZE => Ok(Self::V2),
            BITMAPV3INFOHEADER_SIZE => Ok(Self::V3),
            BITMAPV4HEADER_SIZE => Ok(Self::V4),
            BITMAPV5HEADER_SIZE => Ok(Self::V5),
            // Both of these are "well-formed but not implemented", which is
            // exactly what `Unsupported` means; `BadLength` would imply the
            // field is self-contradictory, and for the 12-byte case it is not.
            BITMAPCOREHEADER_SIZE => Err(Error::new(ErrorKind::Unsupported, 0)),
            _ => Err(Error::new(ErrorKind::Unsupported, 0)),
        }
    }

    /// `biSize` for this version.
    #[must_use]
    pub const fn size(self) -> u32 {
        match self {
            Self::Info => BITMAPINFOHEADER_SIZE,
            Self::V2 => BITMAPV2INFOHEADER_SIZE,
            Self::V3 => BITMAPV3INFOHEADER_SIZE,
            Self::V4 => BITMAPV4HEADER_SIZE,
            Self::V5 => BITMAPV5HEADER_SIZE,
        }
    }
}

/// One channel's extraction rule: a mask plus the shift and width derived from
/// it, precomputed so the per-pixel path is two shifts and a multiply.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct ChannelMask {
    mask: u32,
    shift: u32,
    bits: u32,
}

impl ChannelMask {
    /// A channel that is not present. Extracts as zero.
    pub const NONE: Self = Self {
        mask: 0,
        shift: 0,
        bits: 0,
    };

    /// Build from a raw `DWORD` mask without validating contiguity.
    #[must_use]
    pub const fn new(mask: u32) -> Self {
        if mask == 0 {
            return Self::NONE;
        }
        let shift = mask.trailing_zeros();
        let bits = mask.count_ones();
        Self { mask, shift, bits }
    }

    /// Build from a raw mask, rejecting anything the extraction rule cannot
    /// express.
    ///
    /// The spec requires the set bits of each mask to be contiguous. A
    /// non-contiguous mask has no single shift-and-scale that reproduces it, so
    /// silently using `trailing_zeros`/`count_ones` would emit wrong colours
    /// with no indication anything went wrong. A mask wider than `bit_count`
    /// addresses bits that are not in the pixel at all.
    pub fn checked(mask: u32, bit_count: u16, offset: usize) -> Result<Self> {
        let m = Self::new(mask);
        if mask != 0 {
            if (mask >> m.shift) != Self::span(m.bits) {
                return Err(Error::new(ErrorKind::Malformed, offset));
            }
            if bit_count < 32 && (mask >> bit_count) != 0 {
                return Err(Error::new(ErrorKind::Malformed, offset));
            }
        }
        Ok(m)
    }

    const fn span(bits: u32) -> u32 {
        if bits >= 32 {
            u32::MAX
        } else {
            (1u32 << bits) - 1
        }
    }

    /// The raw `DWORD` mask.
    #[must_use]
    pub const fn mask(self) -> u32 {
        self.mask
    }

    /// Number of set bits, i.e. this channel's precision in the source pixel.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.bits
    }

    /// Whether the channel is present at all.
    #[must_use]
    pub const fn is_present(self) -> bool {
        self.mask != 0
    }

    /// Pull this channel out of a packed pixel and rescale it to 8 bits.
    ///
    /// Rescaling is `round(v * 255 / max)` rather than a left shift, because a
    /// left shift maps the 5-bit maximum 31 to 248 instead of 255 — a
    /// full-white RGB555 image would decode to a slightly grey one.
    #[must_use]
    pub const fn extract(self, pixel: u32) -> u8 {
        if self.bits == 0 {
            return 0;
        }
        let v = (pixel & self.mask) >> self.shift;
        if self.bits == 8 {
            return v as u8;
        }
        let max = Self::span(self.bits) as u64;
        (((v as u64) * 255 + max / 2) / max) as u8
    }
}

/// The four channel masks in play for one image.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct Masks {
    /// `biRedMask` / `bV5RedMask`, or the depth's `BI_RGB` default.
    pub red: ChannelMask,
    /// `biGreenMask` / `bV5GreenMask`, or the depth's `BI_RGB` default.
    pub green: ChannelMask,
    /// `biBlueMask` / `bV5BlueMask`, or the depth's `BI_RGB` default.
    pub blue: ChannelMask,
    /// `bV5AlphaMask`, or absent. Never defaulted from the bit depth — see
    /// [`Masks::for_bi_rgb`].
    pub alpha: ChannelMask,
}

impl Masks {
    /// The layouts `BI_RGB` implies for each bit depth.
    ///
    /// Note what is *not* here: 32-bpp `BI_RGB` gets no alpha mask. Per the
    /// docs the high byte of each `DWORD` "is not used" — it is not a
    /// transparency channel, and treating it as one turns every screenshot
    /// pasted as `CF_DIB` into a fully transparent image, because a great many
    /// producers leave that byte at zero.
    #[must_use]
    pub const fn for_bi_rgb(bit_count: u16) -> Self {
        match bit_count {
            // RGB555: five bits each, blue in the low bits, top bit unused.
            16 => Self {
                red: ChannelMask::new(0x0000_7C00),
                green: ChannelMask::new(0x0000_03E0),
                blue: ChannelMask::new(0x0000_001F),
                alpha: ChannelMask::NONE,
            },
            32 => Self {
                red: ChannelMask::new(0x00FF_0000),
                green: ChannelMask::new(0x0000_FF00),
                blue: ChannelMask::new(0x0000_00FF),
                alpha: ChannelMask::NONE,
            },
            // 1/4/8 go through the palette and 24 is raw BGR bytes; neither
            // consults a mask.
            _ => Self {
                red: ChannelMask::NONE,
                green: ChannelMask::NONE,
                blue: ChannelMask::NONE,
                alpha: ChannelMask::NONE,
            },
        }
    }

    /// Bits that no colour channel claims. Used to decide whether there is
    /// somewhere an undeclared alpha channel could be hiding.
    #[must_use]
    pub const fn uncovered(self) -> u32 {
        !(self.red.mask | self.green.mask | self.blue.mask | self.alpha.mask)
    }
}

/// A parsed DIB information header plus the byte layout derived from it.
///
/// Every offset and length in here has already been checked against the source
/// buffer, so decoding is a matter of walking rows, not of re-validating
/// fields. Construct with [`DibHeader::parse`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct DibHeader {
    version: HeaderVersion,
    width: u32,
    height: u32,
    top_down: bool,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    clr_used: u32,
    clr_important: u32,
    color_space: u32,
    endpoints: Endpoints,
    gamma: Gamma,
    masks: Masks,
    rle: bool,
    palette_offset: usize,
    palette_entries: u32,
    pixel_offset: usize,
    stride: usize,
    image_bytes: usize,
    rgba_len: usize,
}

impl DibHeader {
    /// Parse and fully validate the header of a packed DIB.
    ///
    /// `src` is the whole clipboard payload: information header, then masks
    /// and/or palette, then pixels. There is no `BITMAPFILEHEADER` and none is
    /// expected — that 14-byte prefix belongs to `.bmp` files, not to
    /// `CF_DIB`/`CF_DIBV5`.
    ///
    /// On success every derived offset is known to be in bounds for this exact
    /// buffer, which is why [`Self::decode_into`](crate::DibHeader::decode_into)
    /// insists on being handed the same one.
    pub fn parse(src: &[u8]) -> Result<Self> {
        let mut r = Reader::new(src);
        let header_size = r.u32_le()?;
        let version = HeaderVersion::from_size(header_size)?;

        // Offsets 4..40 are byte-identical in every version from 40 up, which
        // is the whole point of the `biSize` discriminator.
        let width_raw = r.i32_le()?;
        let height_raw = r.i32_le()?;
        let planes = r.u16_le()?;
        let bit_count = r.u16_le()?;
        let compression = r.u32_le()?;
        let size_image = r.u32_le()?;
        let x_pels_per_meter = r.i32_le()?;
        let y_pels_per_meter = r.i32_le()?;
        let clr_used = r.u32_le()?;
        let clr_important = r.u32_le()?;
        debug_assert_eq!(
            r.pos(),
            40,
            "the common prefix of every DIB header is exactly 40 bytes"
        );

        if width_raw <= 0 {
            // Width is a LONG but has no negative meaning (only *height* encodes
            // orientation in its sign), so a negative width is nonsense rather
            // than an orientation flag.
            return Err(Error::new(ErrorKind::Malformed, 4));
        }
        if height_raw == 0 {
            return Err(Error::new(ErrorKind::Malformed, 8));
        }
        if planes != 1 {
            // GDI requires 1. A different plane count means a planar pixel
            // layout this decoder does not implement, and guessing would emit
            // scrambled colour rather than fail.
            return Err(Error::new(ErrorKind::Malformed, 12));
        }

        let width = width_raw as u32;
        // Negative biHeight is the top-down flag. `unsigned_abs` and not
        // `-h as u32`: `i32::MIN` has no positive counterpart and negating it
        // overflows.
        let top_down = height_raw < 0;
        let height = height_raw.unsigned_abs();

        // THE guard. A 40-byte header can claim 2^31 x 2^31; refuse before any
        // arithmetic sized by the product, let alone an allocation.
        let pixel_count = u64::from(width) * u64::from(height);
        if pixel_count > rclip_core::MAX_PIXELS {
            return Err(Error::new(ErrorKind::TooLarge, 4));
        }

        match compression {
            BI_RGB | BI_BITFIELDS | BI_ALPHABITFIELDS | BI_RLE8 | BI_RLE4 => {}
            // An embedded JPEG or PNG stream in a DIB wrapper. Out of scope on
            // purpose: those formats have good decoders already.
            BI_JPEG | BI_PNG => return Err(Error::new(ErrorKind::Unsupported, 16)),
            // Anything else is a video FOURCC (the DirectShow reuse of this
            // struct), which is not a bitmap at all.
            _ => return Err(Error::new(ErrorKind::Unsupported, 16)),
        }

        match bit_count {
            1 | 4 | 8 | 16 | 24 | 32 => {}
            // 0 is only legal with BI_JPEG/BI_PNG, which are already rejected.
            _ => return Err(Error::new(ErrorKind::Unsupported, 14)),
        }

        // "The BI_RLE8 value is valid only for 8-bpp bitmaps", likewise
        // BI_RLE4 and 4-bpp. The pairing is not a formality: the run grammars
        // differ, and reading an RLE4 stream as RLE8 walks a different number
        // of pixels per byte and desynchronises immediately.
        let rle = match compression {
            BI_RLE8 => {
                if bit_count != 8 {
                    return Err(Error::new(ErrorKind::Malformed, 14));
                }
                true
            }
            BI_RLE4 => {
                if bit_count != 4 {
                    return Err(Error::new(ErrorKind::Malformed, 14));
                }
                true
            }
            _ => false,
        };
        if rle && top_down {
            // MS-WMF and the GDI docs both say it, in the same words: a
            // top-down DIB cannot be compressed. The run grammar has no way to
            // express which end it started from, so an encoder that set both
            // is contradicting itself and the rows would come out inverted.
            return Err(Error::new(ErrorKind::Malformed, 8));
        }

        let bitfields = compression == BI_BITFIELDS || compression == BI_ALPHABITFIELDS;
        if bitfields && bit_count != 16 && bit_count != 32 {
            // "valid when used with 16- and 32-bpp bitmaps" — a masked 8-bpp
            // image would have to mean palette indices, which is meaningless.
            return Err(Error::new(ErrorKind::Malformed, 16));
        }
        // Where the masks live is the 12-byte trap. With a 40-byte header they
        // follow it, occupying the space a palette would; from 52 bytes up they
        // are fields *inside* the header and nothing follows it. Reading them
        // from the wrong place shifts every subsequent offset.
        let mut extra_after_header = 0usize;
        let mut raw = [0u32; 4];
        match version {
            HeaderVersion::Info => {
                if bitfields {
                    let n = if compression == BI_ALPHABITFIELDS {
                        4
                    } else {
                        3
                    };
                    for slot in raw.iter_mut().take(n) {
                        *slot = r.u32_le()?;
                    }
                    extra_after_header = n * 4;
                }
            }
            _ => {
                // 52 bytes carries three masks, 56 and up carry four.
                raw[0] = r.u32_le()?;
                raw[1] = r.u32_le()?;
                raw[2] = r.u32_le()?;
                if version >= HeaderVersion::V3 {
                    raw[3] = r.u32_le()?;
                }
            }
        }

        // bV4CSType / bV5CSType sits at offset 56, immediately after the four
        // masks, and is followed by the colour-management block: a
        // CIEXYZTRIPLE at 60..96 and three gamma DWORDs at 96..108.
        //
        // TODO(phase-2): the V5 ICC profile — bV5ProfileData (112) and
        // bV5ProfileSize (116) — is read past but not reported. It is a byte
        // range elsewhere in the payload rather than a value, and resolving it
        // means handing a caller an offset that came off the wire.
        let (color_space, endpoints, gamma) = if version >= HeaderVersion::V4 {
            let cs = r.u32_le()?;
            let mut xyz = [CieXyz::default(); 3];
            for slot in &mut xyz {
                slot.x = r.i32_le()?;
                slot.y = r.i32_le()?;
                slot.z = r.i32_le()?;
            }
            let g = Gamma {
                red: r.u32_le()?,
                green: r.u32_le()?,
                blue: r.u32_le()?,
            };
            debug_assert_eq!(r.pos(), BITMAPV4HEADER_SIZE as usize);
            (
                cs,
                Endpoints {
                    red: xyz[0],
                    green: xyz[1],
                    blue: xyz[2],
                },
                g,
            )
        } else {
            (LCS_CALIBRATED_RGB, Endpoints::default(), Gamma::default())
        };

        let mut masks = if bitfields {
            // Both storage sites start at byte 40 of the payload: inside the
            // header for V2 and up, immediately after it for a 40-byte header.
            const MASK_OFFSET: usize = BITMAPINFOHEADER_SIZE as usize;
            let at = MASK_OFFSET;
            let m = Masks {
                red: ChannelMask::checked(raw[0], bit_count, at)?,
                green: ChannelMask::checked(raw[1], bit_count, at + 4)?,
                blue: ChannelMask::checked(raw[2], bit_count, at + 8)?,
                alpha: ChannelMask::checked(raw[3], bit_count, at + 12)?,
            };
            if !m.red.is_present() || !m.green.is_present() || !m.blue.is_present() {
                // BI_BITFIELDS with a zero colour mask cannot produce a colour.
                return Err(Error::new(ErrorKind::Malformed, at));
            }
            m
        } else {
            Masks::for_bi_rgb(bit_count)
        };

        // The one field that survives BI_RGB: unlike bV5RedMask and friends,
        // the docs do not qualify bV5AlphaMask with "valid only if
        // BI_BITFIELDS". V4/V5 producers (Chrome, Firefox) rely on that and set
        // an alpha mask under BI_RGB.
        if !bitfields && version >= HeaderVersion::V3 {
            masks.alpha = ChannelMask::checked(raw[3], bit_count, 52)?;
        }

        // biClrUsed == 0 means "the maximum for this depth". At 16/24/32 bpp a
        // non-zero count is a palette-optimisation hint, but it still occupies
        // bytes between the header and the pixels, so it counts towards the
        // pixel offset either way.
        let palette_entries = if bit_count <= 8 {
            if clr_used == 0 {
                1u32 << bit_count
            } else {
                clr_used
            }
        } else {
            clr_used
        };

        let header_size_us =
            usize::try_from(header_size).map_err(|_| r.err(ErrorKind::TooLarge))?;
        let palette_offset = header_size_us
            .checked_add(extra_after_header)
            .ok_or_else(|| r.err(ErrorKind::TooLarge))?;
        let palette_bytes = usize::try_from(u64::from(palette_entries) * 4)
            .map_err(|_| Error::new(ErrorKind::TooLarge, 32))?;
        let pixel_offset = palette_offset
            .checked_add(palette_bytes)
            .ok_or(Error::new(ErrorKind::TooLarge, 32))?;

        // Bounds-check the whole layout once, here, so the decoder never has to.
        let avail = src.len().checked_sub(pixel_offset).ok_or(Error::new(
            ErrorKind::BadOffset,
            pixel_offset.min(src.len()),
        ))?;

        let (stride, image_bytes) = if rle {
            // A run-length stream has no fixed row stride at all, so there is
            // no `stride * height` to derive: the only statement of how many
            // bytes belong to the image is `biSizeImage`, which the docs make
            // mandatory for BI_RLE4/BI_RLE8 for exactly this reason.
            //
            // Zero is nonetheless common from producers that copied the
            // "may be zero for BI_RGB" rule across, so it is read as "the rest
            // of the payload" rather than as an empty image. A non-zero value
            // that does not fit is a truncated payload, not something to clamp:
            // the decoder would stop mid-run and emit a half-drawn image.
            let declared =
                usize::try_from(size_image).map_err(|_| Error::new(ErrorKind::TooLarge, 20))?;
            if declared == 0 {
                (0, avail)
            } else if declared > avail {
                return Err(Error::new(ErrorKind::UnexpectedEof, src.len()));
            } else {
                (0, declared)
            }
        } else {
            // Rows are padded to a 4-byte boundary. This is the off-by-one that
            // skews an image diagonally instead of failing: at 24 bpp a 3-pixel
            // row is 9 bytes of colour but 12 bytes on the wire.
            let stride_u64 = (u64::from(width) * u64::from(bit_count)).div_ceil(32) * 4;
            let image_bytes_u64 = stride_u64 * u64::from(height);
            let stride =
                usize::try_from(stride_u64).map_err(|_| Error::new(ErrorKind::TooLarge, 4))?;
            let image_bytes =
                usize::try_from(image_bytes_u64).map_err(|_| Error::new(ErrorKind::TooLarge, 4))?;
            if image_bytes > avail {
                // Truncated payload. Some producers omit the final row's
                // padding; this decoder does not paper over that, because the
                // alternative is emitting a partly uninitialised last row.
                return Err(Error::new(ErrorKind::UnexpectedEof, src.len()));
            }
            (stride, image_bytes)
        };

        // Post-guard arithmetic: pixel_count is already <= MAX_PIXELS, so this
        // cannot exceed 4 * MAX_PIXELS on any target with a 32-bit usize.
        let rgba_len =
            usize::try_from(pixel_count * 4).map_err(|_| Error::new(ErrorKind::TooLarge, 4))?;

        Ok(Self {
            version,
            width,
            height,
            top_down,
            bit_count,
            compression,
            size_image,
            x_pels_per_meter,
            y_pels_per_meter,
            clr_used,
            clr_important,
            color_space,
            endpoints,
            gamma,
            masks,
            rle,
            palette_offset,
            palette_entries,
            pixel_offset,
            stride,
            image_bytes,
            rgba_len,
        })
    }

    /// Which information header this payload carried.
    #[must_use]
    pub const fn version(self) -> HeaderVersion {
        self.version
    }

    /// Image width in pixels. Always positive.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Image height in pixels, as an absolute value. The sign of `biHeight` is
    /// reported separately by [`Self::is_top_down`].
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// `true` when `biHeight` was negative: rows are stored first-to-last with
    /// the origin at the top-left. `false` — the format's default — means rows
    /// are stored bottom-to-top.
    #[must_use]
    pub const fn is_top_down(self) -> bool {
        self.top_down
    }

    /// `biBitCount`: 1, 4, 8, 16, 24 or 32.
    #[must_use]
    pub const fn bit_count(self) -> u16 {
        self.bit_count
    }

    /// `biCompression`, one of [`BI_RGB`], [`BI_BITFIELDS`],
    /// [`BI_ALPHABITFIELDS`].
    #[must_use]
    pub const fn compression(self) -> u32 {
        self.compression
    }

    /// `biSizeImage` as declared. Informational only: it is routinely zero for
    /// uncompressed bitmaps, so the decoder derives the real size from the
    /// stride instead of trusting this.
    #[must_use]
    pub const fn declared_size_image(self) -> u32 {
        self.size_image
    }

    /// `biXPelsPerMeter`.
    #[must_use]
    pub const fn x_pels_per_meter(self) -> i32 {
        self.x_pels_per_meter
    }

    /// `biYPelsPerMeter`.
    #[must_use]
    pub const fn y_pels_per_meter(self) -> i32 {
        self.y_pels_per_meter
    }

    /// `biClrUsed` as declared.
    #[must_use]
    pub const fn declared_clr_used(self) -> u32 {
        self.clr_used
    }

    /// `biClrImportant` as declared.
    #[must_use]
    pub const fn clr_important(self) -> u32 {
        self.clr_important
    }

    /// `bV4CSType`/`bV5CSType`, or [`LCS_CALIBRATED_RGB`] for headers that have
    /// no such field. This crate reports the colour space but does not convert:
    /// the pixels come out in whatever space they went in.
    #[must_use]
    pub const fn color_space(self) -> u32 {
        self.color_space
    }

    /// `bV4Endpoints`/`bV5Endpoints`, the `CIEXYZTRIPLE` at bytes 60..96, for
    /// a header that has one. `None` below [`HeaderVersion::V4`].
    ///
    /// **Reported, never applied.** The endpoints and [`Self::gamma`] together
    /// describe the RGB primaries the producer's pixels are in, and are only
    /// *meaningful* when [`Self::color_space`] is [`LCS_CALIBRATED_RGB`] —
    /// every other value names a colour space that supersedes them. This crate
    /// deliberately performs no conversion: a clipboard decoder that
    /// half-applies a colour transform produces pixels that are wrong in a way
    /// nothing downstream can detect or undo, whereas passing them through
    /// unchanged leaves a caller with an ICC-shaped problem it can still solve.
    #[must_use]
    pub const fn endpoints(self) -> Option<Endpoints> {
        match self.version {
            HeaderVersion::V4 | HeaderVersion::V5 => Some(self.endpoints),
            _ => None,
        }
    }

    /// `bV4GammaRed`/`Green`/`Blue`, the three 16.16 `DWORD`s at bytes 96..108,
    /// for a header that has them. `None` below [`HeaderVersion::V4`].
    ///
    /// Reported, never applied — see [`Self::endpoints`].
    #[must_use]
    pub const fn gamma(self) -> Option<Gamma> {
        match self.version {
            HeaderVersion::V4 | HeaderVersion::V5 => Some(self.gamma),
            _ => None,
        }
    }

    /// Whether [`Self::endpoints`] and [`Self::gamma`] describe this image, i.e.
    /// `bV5CSType == LCS_CALIBRATED_RGB` on a header that carries them.
    #[must_use]
    pub const fn is_calibrated(self) -> bool {
        self.color_space == LCS_CALIBRATED_RGB && self.endpoints().is_some()
    }

    /// The channel masks in effect, whether they were read from the header,
    /// from the DWORDs following it, or defaulted from the bit depth.
    #[must_use]
    pub const fn masks(self) -> Masks {
        self.masks
    }

    /// Whether the pixel data is a [`BI_RLE4`] or [`BI_RLE8`] run-length stream
    /// rather than packed rows.
    #[must_use]
    pub const fn is_rle(self) -> bool {
        self.rle
    }

    /// Number of `RGBQUAD` entries between the header and the pixels.
    #[must_use]
    pub const fn palette_entries(self) -> u32 {
        self.palette_entries
    }

    /// Byte offset of the palette within the payload.
    #[must_use]
    pub const fn palette_offset(self) -> usize {
        self.palette_offset
    }

    /// Byte offset of the first row of pixel data within the payload.
    #[must_use]
    pub const fn pixel_offset(self) -> usize {
        self.pixel_offset
    }

    /// Bytes per stored row, including the pad to a 4-byte boundary.
    ///
    /// **Zero for an RLE image**, which has no fixed row stride: a compressed
    /// row is whatever length its runs came to. Zero rather than the stride the
    /// *decoded* rows would have, so that arithmetic written for the packed
    /// case cannot quietly produce a plausible wrong answer here.
    #[must_use]
    pub const fn stride(self) -> usize {
        self.stride
    }

    /// Total bytes of pixel data: `stride * height` for a packed image, and the
    /// length of the compressed stream for an RLE one.
    #[must_use]
    pub const fn image_bytes(self) -> usize {
        self.image_bytes
    }

    /// Size in bytes of the RGBA8 buffer [`Self::decode_into`] needs:
    /// `width * height * 4`.
    ///
    /// [`Self::decode_into`]: crate::DibHeader::decode_into
    #[must_use]
    pub const fn required_buffer_len(self) -> usize {
        self.rgba_len
    }

    /// Whether a palette is consulted when decoding, i.e. `bit_count <= 8`.
    #[must_use]
    pub const fn is_palettised(self) -> bool {
        self.bit_count <= 8
    }
}

# rclip-dib

`CF_DIB` and `CF_DIBV5` — Windows *packed* device-independent bitmaps — decoded to RGBA8, and
RGBA8 encoded back to `CF_DIBV5`. Packed means there is no 14-byte `BITMAPFILEHEADER`: the
clipboard payload starts at the information header, there is no `BM` magic and no explicit
offset to the pixels, so the entire layout has to be derived from the header's own fields. That
is the single reason a BMP crate written for `.bmp` files cannot be pointed at this data.

Normative sources, all field offsets verified against them rather than from memory:

- [BITMAPINFOHEADER](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmapinfoheader)
  — 40 bytes, `CF_DIB`.
- [BITMAPV4HEADER](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmapv4header)
  — 108 bytes.
- [BITMAPV5HEADER](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmapv5header)
  — 124 bytes, `CF_DIBV5`.
- [RGBQUAD](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-rgbquad) (blue
  first) and [CIEXYZTRIPLE](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-ciexyztriple)
  (three `CIEXYZ`, each three 4-byte `FXPT2DOT30` — 36 bytes, which is what makes V4 108 and V5 124).

## Shape

`#![no_std]`, `#![forbid(unsafe_code)]`, no dependency but `rclip-core`.

```rust
let header = DibHeader::parse(payload)?;                 // validates the whole layout
let mut rgba = vec![0u8; header.required_buffer_len()];  // width * height * 4
header.decode_into(payload, &mut rgba, AlphaMode::Straight)?;
```

`decode_into` writes into a caller-provided buffer and allocates nothing. `decode` and
`encode_v5`, which return owned buffers, sit behind the `alloc` feature; `encode_v5_into` is the
borrowed-buffer encoder and is available without it.

## The alpha policy

`CF_DIBV5` has no agreed alpha convention and carries no in-band signal for one. Chromium and
Firefox write **premultiplied** RGBA; XnView and Photoshop read the same bytes as **straight**.
So the mode is a caller-supplied policy, never a silent guess:

```rust
pub enum AlphaMode { Straight, Premultiplied, Guess }
```

`Guess` is a heuristic and is documented as one. The only sound inference available is
one-directional: a pixel whose red, green or blue exceeds its alpha cannot have been produced by
premultiplication, so an image containing one is straight. The converse does not hold — a dark
straight-alpha image satisfies `c <= a` everywhere and gets classified as premultiplied and
wrongly brightened. Pass an explicit mode whenever the source application is known.

Separately, a 32-bpp `BITMAPINFOHEADER` (not V4/V5) technically has **no** alpha channel at all:
the docs say the high byte of each `DWORD` "is not used", so its contents are undefined and
producers routinely leave it at zero. Honouring it blindly turns every pasted screenshot into a
fully transparent image. This crate treats it as alpha only when *every* pixel's fourth byte is
non-zero. Both cases are fixtures:
`32bpp-info-alpha-all-set-2x1` and `32bpp-info-alpha-zero-2x1`.

## The traps this crate exists to get right

- **Header size discriminates the variant.** 40 / 108 / 124, plus the two undocumented Adobe
  sizes 52 and 56. An unrecognised size is `Unsupported`, never rounded down — a wrong guess
  shifts the pixel data and produces a skewed image rather than an error.
- **`biHeight` negative means top-down**; positive (the default) means bottom-up, rows stored
  last-first. Read with `unsigned_abs`, because negating `i32::MIN` overflows.
- **4-byte row stride.** `((width * bpp + 31) / 32) * 4`. Off by one and the whole image skews.
- **`BI_BITFIELDS` mask placement.** With a 40-byte header three (or four, for the Windows CE
  `BI_ALPHABITFIELDS`) `DWORD` masks *follow* the header, before the pixels. With V2 and up they
  are fields *inside* it. Getting this wrong shifts the pixel data by 12 bytes; there is a
  fixture for each direction (`16bpp-bitfields-565-2x2`, `32bpp-v4-bitfields-2x1`).
- **`bV5AlphaMask` survives `BI_RGB`.** Unlike the colour masks, the docs never qualify it with
  "valid only if BI_BITFIELDS", and V4/V5 producers rely on that.
- **`RGBQUAD` is blue-first**, and `rgbReserved` is documented as "must be zero" — it is not an
  alpha channel, so palettised images decode opaque.
- **`biClrUsed == 0` means `1 << bpp` entries.** A non-zero `biClrUsed` at 16/24/32 bpp is only a
  palette-optimisation hint, but those entries still occupy bytes before the pixels and are
  counted towards the pixel offset.
- **Channels rescale by rounding, not shifting.** `round(v * 255 / max)`; a left shift maps the
  5-bit maximum 31 to 248, so a full-white RGB555 image would decode slightly grey.
- **`MAX_PIXELS` is checked before any arithmetic sized by the dimensions.** A 40-byte header can
  claim 65536 x 65536 — sixteen gigabytes of RGBA. That is `ErrorKind::TooLarge`, not an
  allocation (`huge-dimensions` fixture).

## Prior art

Checked before writing anything. The disqualifier for most of them is the same: a *packed* DIB
has no `BITMAPFILEHEADER`, and BMP crates are written for `.bmp` files.

- **[`image`](https://crates.io/crates/image)** (`codecs::bmp`) — the only one that handles this
  properly: `BmpDecoder::new_without_file_header` exists specifically for `CF_DIB`, and it reads
  V4/V5 masks including alpha. Rejected on cost and policy: it needs `std` (`BufRead + Seek`),
  pulls `bytemuck`, `byteorder-lite`, `moxcms` and `num-traits` unconditionally plus the whole
  `image` type system, has no premultiplied-alpha concept at all (it treats every alpha as
  straight), and `PLAN.md` scopes PNG/JPEG/TIFF out of this workspace precisely so `image` stays
  an optional consumer-side choice rather than a hard dependency of the clipboard layer.
- **[`tinybmp`](https://crates.io/crates/tinybmp)** 0.7 — genuinely `no_std` and the closest fit
  on paper, but its `Header` carries `file_size` and `image_data_start`, i.e. it parses a
  `BITMAPFILEHEADER` a clipboard payload does not have. It also depends on `embedded-graphics`
  0.8 and exposes no header-version or alpha-convention notion.
- **[`dib`](https://crates.io/crates/dib)** 0.1.0 — right API shape (`decode`, `decode_into`,
  `encode`, `encoded_size`), wrong everything else: one release from April 2024, ~1.7k downloads,
  three obscure micro-crate dependencies (`atools`, `bites`, `car`), no documented `no_std`
  support, and nothing about header versions or the alpha convention.
- **[`bmp`](https://crates.io/crates/bmp)** 0.5 — `std`-only, file-oriented (`Image::open`), last
  released 2019, 24-bpp only, requires the file header.
- **`bmp-rust`** — parses "the file header, DIB header and other parts of the file";
  file-header-oriented again, and unmaintained.
- **[`windows`](https://crates.io/crates/windows) / `windows-sys`** — supply the `BITMAPV5HEADER`
  *struct definition* and nothing that decodes it, are Windows-only, and are enormous. Useful as
  a cross-check on field order; not a decoder.

Verdict: written from scratch against the Microsoft docs. The reusable part of the job was a
tenth of the crate, and none of the candidates handled the alpha question, which is the part that
actually breaks in production.

## Not implemented

- `BI_RLE4` / `BI_RLE8` — `// TODO(phase-1)` in `header.rs`. Reported as `Unsupported`. No modern
  clipboard producer writes RLE; if a real capture ever shows one, it lands here.
- `BI_JPEG` / `BI_PNG` — permanently out of scope. These are an embedded JPEG or PNG stream in a
  DIB wrapper; `PLAN.md` §4.4 delegates those formats to `image` or azul's existing decoders.
  Reported as `Unsupported`.
- 12-byte `BITMAPCOREHEADER` — 16-bit dimensions and a 3-byte `RGBTRIPLE` palette, an entirely
  different layout. Rejected as `Unsupported` rather than misread as a 40-byte header.
- ICC profiles and colour management — `bV5CSType` is reported via `color_space()`, but the
  endpoints, gamma and `bV5ProfileData` are not extracted and no transform is applied. Pixels
  come out in whatever space they went in. `// TODO(phase-2)` in `header.rs`.
- The encoder emits exactly one shape: 32-bpp `BI_BITFIELDS` `BITMAPV5HEADER`, bottom-up, sRGB.
  No palettised, 16-bpp or 24-bpp output, and no `CF_DIB` (V1) output — a producer has no reason
  to write a format that cannot carry alpha.
- `CF_BITMAP` is a device-dependent `HBITMAP` *handle*, not a byte format, so it is not something
  a parser crate can own.

## Fixtures

`corpus/synthetic/rclip-dib/`, hand-built to the documented field order, each with a `.json`
sidecar. Ten `ok` cases covering 1/4/8/16/24/32 bpp, both row orders, both `BI_BITFIELDS` mask
placements and both 32-bpp-alpha outcomes; four `error` cases — `huge-dimensions`,
`bitmapcoreheader-12`, `bad-header-size-64` and `24bpp-truncated-2x2`. The undocumented 52/56-byte
headers and the header-field malformations (`biPlanes != 1`, non-contiguous masks, RLE
compression) are built inline in `tests/dib.rs`, since they are one-field variations rather than
distinct payload shapes.

The integration tests also feed every prefix of every fixture through the parser and decoder, to
prove that no input truncation panics.

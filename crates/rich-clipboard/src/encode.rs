//! [`RichItem`] to a set of flavors.
//!
//! The direction that decides whether a paste lands in Word as styled text or
//! as a flat string. See [`crate::fanout`] for the table this walks, and for
//! why it is not the read table reversed.

use alloc::string::String;
use alloc::vec::Vec;

use rclip_core::{ClipboardItem, ClipboardPayload, Flavor, Platform};

use crate::decode::Options;
use crate::error::{Error, Result};
use crate::fanout::{write_plan, WriteFlavor};
use crate::item::{Image, ImageFormat, RichItem, TransferAction};
use crate::native::native_name;
use crate::text;

/// Publish an item as every flavor it can be expressed in on `platform`.
///
/// The result is ordered best-first, and that order is what a transport hands
/// to `SetClipboardData` / `writeObjects:` / a `TARGETS` reply.
///
/// A flavor in the plan that this build cannot produce is skipped, not fatal: a
/// build without the `rtf` feature still publishes HTML and plain text on
/// Windows, and the paste still works — it is just worse in WordPad. Compare
/// against [`write_plan`] to see what a full build would have offered.
///
/// # Errors
///
/// [`Error::NotPublishable`] if the item has no representation on `platform` at
/// all, and [`Error::NothingEncodable`] if it has one in principle but every
/// flavor needed a feature this build lacks — the error names the feature.
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "std", feature = "file-list"))] {
/// use rclip_core::Platform;
/// use rich_clipboard::{encode, FileList, RichItem};
///
/// let files = FileList::of_paths(["/home/me/a.txt"]);
/// let payload = encode(&RichItem::Files(files), Platform::Unix).unwrap();
///
/// // All three Linux conventions, because none of them reads the others'.
/// let natives: Vec<_> = payload.items().iter().map(|i| i.native.as_str()).collect();
/// assert!(natives.contains(&"text/uri-list"));
/// assert!(natives.contains(&"x-special/gnome-copied-files"));
/// # }
/// ```
pub fn encode(item: &RichItem, platform: Platform) -> Result<ClipboardPayload> {
    encode_with(item, platform, &Options::default())
}

/// Publish an item, with explicit policy.
///
/// The alpha mode in `options` is read here too, and it means something
/// different in this direction: it says how alpha is spelled on the *wire*, not
/// what [`RgbaImage`](crate::RgbaImage) holds — that is always straight.
/// `Guess` has no meaning for a writer, so it becomes straight, which is the
/// convention the largest number of Windows readers assume.
///
/// # Errors
///
/// As [`encode`].
pub fn encode_with(
    item: &RichItem,
    platform: Platform,
    options: &Options,
) -> Result<ClipboardPayload> {
    // `Unknown` is not in the table: its flavor is not known until run time.
    // Republishing it verbatim under the identifier it arrived with is what
    // makes a clipboard bridge possible.
    if let RichItem::Unknown { native, bytes } = item {
        let mut payload = ClipboardPayload::new(platform);
        payload.push(ClipboardItem::new(native.clone(), bytes.clone()));
        return Ok(payload);
    }

    let kind = item.kind();
    let plan = write_plan(kind, platform);
    if plan.is_empty() {
        return Err(Error::NotPublishable { kind, platform });
    }

    let mut payload = ClipboardPayload::new(platform);
    let mut missing: Option<&'static str> = None;
    for entry in plan {
        let Some(native) = native_name(entry.flavor, platform) else {
            continue;
        };
        let native = String::from(native);
        match encode_flavor(item, entry, platform, options) {
            Ok(emitted) => {
                for (n, bytes) in emitted.blobs.into_iter().enumerate() {
                    let index = if emitted.per_item { n } else { 0 };
                    payload.push(ClipboardItem::in_item(index, native.clone(), bytes));
                }
            }
            Err(feature) => missing = missing.or(Some(feature)),
        }
    }

    if payload.is_empty() {
        return Err(Error::NothingEncodable {
            kind,
            platform,
            missing,
        });
    }
    Ok(payload)
}

/// What one flavor contributes to the payload.
///
/// Usually zero blobs (the flavor does not apply to this item) or one. It is a
/// `Vec` because of macOS: a pasteboard models a multi-file selection as N
/// *items*, each carrying one `public.file-url`, and there is no other way to
/// say "these four files" there.
///
/// `per_item` is the difference between those two situations, and it is a
/// property of the *flavor*, not of the payload. Four `public.file-url` blobs
/// are four items; four blobs under any other flavor would be one item
/// advertising the same type four times, which is malformed. Only the encoder
/// that produced them knows which it meant, so it says.
struct Emitted {
    blobs: Vec<Vec<u8>>,
    /// One pasteboard item per blob, rather than all of them in item 0.
    per_item: bool,
}

/// `Err` names the Cargo feature whose codec was missing.
type FlavorResult = core::result::Result<Emitted, &'static str>;

fn grouped(blobs: Vec<Vec<u8>>) -> FlavorResult {
    Ok(Emitted {
        blobs,
        per_item: false,
    })
}

fn none() -> FlavorResult {
    grouped(Vec::new())
}

/// Every caller is behind a format feature; a build with none of them on
/// publishes only plain text, which goes through [`maybe`].
#[cfg_attr(not(feature = "full"), allow(dead_code))]
fn one(bytes: Vec<u8>) -> FlavorResult {
    grouped(alloc::vec![bytes])
}

fn maybe(bytes: Option<Vec<u8>>) -> FlavorResult {
    grouped(bytes.into_iter().collect())
}

/// One pasteboard item per blob. macOS only — nothing else has items.
#[cfg(feature = "file-list")]
fn per_item(blobs: Vec<Vec<u8>>) -> FlavorResult {
    Ok(Emitted {
        blobs,
        per_item: true,
    })
}

fn encode_flavor(
    item: &RichItem,
    entry: &WriteFlavor,
    platform: Platform,
    options: &Options,
) -> FlavorResult {
    let _ = options;
    match entry.flavor {
        Flavor::PlainText => maybe(item.plain_text().map(|t| text::encode_plain(t, platform))),

        Flavor::Rtf => encode_rtf(item),
        Flavor::Html => encode_html(item, platform),

        Flavor::DibV5 => encode_dib_v5(item, options),
        Flavor::Dib => encode_dib_legacy(item),
        Flavor::Png => maybe(encoded_image_bytes(item, ImageFormat::Png)),
        Flavor::Jpeg => maybe(encoded_image_bytes(item, ImageFormat::Jpeg)),
        Flavor::Gif => maybe(encoded_image_bytes(item, ImageFormat::Gif)),
        Flavor::Tiff => maybe(encoded_image_bytes(item, ImageFormat::Tiff)),

        Flavor::FileList => encode_file_list(item, platform),
        Flavor::DropEffect => maybe(drop_effect_bytes(item)),
        Flavor::FileDescriptor => encode_file_desc(item),

        Flavor::Url => maybe(match item {
            RichItem::Link(link) => Some(text::encode_plain(link.target.as_str(), platform)),
            _ => None,
        }),
        Flavor::UrlName => maybe(match item {
            RichItem::Link(link) => link
                .title
                .as_deref()
                .map(|t| text::encode_plain(t, platform)),
            _ => None,
        }),

        Flavor::Other(name) => encode_convention(item, name),

        // `Flavor` is `#[non_exhaustive]`. A variant added to `rclip-core`
        // after this build cannot appear in a table this build compiled, so
        // there is nothing to do for it.
        _ => none(),
    }
}

/// The bytes for an encoded-image flavor: `public.png`, `PNG`, `public.tiff`.
///
/// Already-encoded bytes are passed through when the format matches, and never
/// transcoded between formats — a PNG on the clipboard is offered as PNG and
/// not as TIFF, because re-encoding one lossless format as another costs time
/// and gains nothing a receiver asked for.
///
/// Pixels are encoded here only with the `image` feature on. `plan/PLAN.md`
/// §4.4 keeps PNG, JPEG, GIF and TIFF encoders out of this *workspace* on
/// purpose — they have good implementations already — and delegates; the
/// `image` feature is that delegation, kept optional so the default dependency
/// graph is unchanged. Without it a consumer holding pixels should encode
/// first and hand over an [`Image::Encoded`].
fn encoded_image_bytes(item: &RichItem, want: ImageFormat) -> Option<Vec<u8>> {
    match item {
        RichItem::Image(Image::Encoded { format, bytes }) if *format == want => Some(bytes.clone()),
        #[cfg(feature = "image")]
        RichItem::Image(Image::Rgba(img)) => encode_pixels(img, want),
        _ => None,
    }
}

/// Encode pixels as `want`, through the `image` crate.
///
/// PNG and TIFF only. They are the two the fan-out table names — `public.png`
/// and `public.tiff` on macOS, `PNG` on Windows, `image/png` on X11 and Wayland
/// — and adding JPEG would mean choosing a quality factor on the user's behalf
/// for a clipboard image that is very often a screenshot of text.
///
/// `None` rather than an error for a dimension mismatch: a caller who built an
/// `RgbaImage` whose buffer does not match its dimensions gets the rest of the
/// fan-out, which is the same treatment every other flavor's bad input gets.
#[cfg(feature = "image")]
fn encode_pixels(img: &crate::item::RgbaImage, want: ImageFormat) -> Option<Vec<u8>> {
    use image::{ExtendedColorType, ImageEncoder};

    let expected = (img.width as usize)
        .checked_mul(img.height as usize)?
        .checked_mul(4)?;
    if img.pixels.len() < expected || img.width == 0 || img.height == 0 {
        return None;
    }
    let pixels = &img.pixels[..expected];

    let mut out = Vec::new();
    match want {
        ImageFormat::Png => {
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(pixels, img.width, img.height, ExtendedColorType::Rgba8)
                .ok()?;
        }
        // The TIFF encoder back-patches its IFD offsets, so it needs `Seek` and
        // therefore a cursor rather than a bare `Vec`.
        ImageFormat::Tiff => {
            let mut cursor = std::io::Cursor::new(&mut out);
            image::codecs::tiff::TiffEncoder::new(&mut cursor)
                .write_image(pixels, img.width, img.height, ExtendedColorType::Rgba8)
                .ok()?;
        }
        _ => return None,
    }
    Some(out)
}

fn drop_effect_bytes(item: &RichItem) -> Option<Vec<u8>> {
    use rclip_core::flavor::drop_effect as fx;

    let RichItem::Files(list) = item else {
        return None;
    };
    let word = match list.action {
        TransferAction::Copy => fx::COPY,
        TransferAction::Move => fx::MOVE,
        TransferAction::Link => fx::LINK,
    };
    Some(Vec::from(word.to_le_bytes()))
}

// ---------------------------------------------------------------------------
// Per-flavor encoders, each present only when its feature is on
// ---------------------------------------------------------------------------

#[cfg(feature = "rtf")]
fn encode_rtf(item: &RichItem) -> FlavorResult {
    match item {
        RichItem::RichText(text) => one(text.to_rtf()),
        // Plain text could be wrapped in an RTF document, but publishing an
        // unstyled string as `Rich Text Format` claims a fidelity it does not
        // have — and the plan for `Text` does not ask for it.
        _ => none(),
    }
}

#[cfg(not(feature = "rtf"))]
fn encode_rtf(_item: &RichItem) -> FlavorResult {
    Err("rtf")
}

#[cfg(feature = "html")]
fn encode_html(item: &RichItem, platform: Platform) -> FlavorResult {
    let (fragment, source_url) = match item {
        RichItem::RichText(text) => (text.to_html_fragment(), None),
        RichItem::Html(html) => (html.markup.clone(), html.source_url.as_deref()),
        _ => return none(),
    };
    if platform == Platform::Windows {
        // `CF_HTML`'s offsets are absolute into the finished blob, which is why
        // this goes through the builder rather than being formatted by hand.
        let mut builder = rclip_cf_html::CfHtmlBuilder::new(&fragment);
        if let Some(url) = source_url {
            builder = builder.source_url(url);
        }
        // A source URL containing a line break would inject a header line, and
        // markup already carrying a `<!--StartFragment-->` would move the
        // boundary a reader finds. Both are the caller's bad data and neither
        // is a reason to fail the whole publish: drop this flavor, keep the
        // rest of the fan-out.
        return maybe(builder.build().ok());
    }
    one(fragment.into_bytes())
}

#[cfg(not(feature = "html"))]
fn encode_html(_item: &RichItem, _platform: Platform) -> FlavorResult {
    Err("html")
}

#[cfg(feature = "dib")]
fn encode_dib_v5(item: &RichItem, options: &Options) -> FlavorResult {
    let RichItem::Image(Image::Rgba(img)) = item else {
        return none();
    };
    let alpha = match options.wire_alpha() {
        rclip_dib::AlphaMode::Premultiplied => rclip_dib::AlphaMode::Premultiplied,
        // `Guess` is a reader's word. For a writer it means nothing, so it
        // becomes `Straight`.
        _ => rclip_dib::AlphaMode::Straight,
    };
    maybe(rclip_dib::encode_v5(img.width, img.height, &img.pixels, alpha).ok())
}

#[cfg(not(feature = "dib"))]
fn encode_dib_v5(_item: &RichItem, _options: &Options) -> FlavorResult {
    Err("dib")
}

/// `CF_DIB`: a 40-byte `BITMAPINFOHEADER`, 24 bpp, for the Win32 applications
/// that read it and not `CF_DIBV5`.
///
/// The format has no alpha channel, so the one decision here is what becomes of
/// it. [`Flatten::OVER_WHITE`] rather than `Discard`: this flavor is the
/// fall-back for a consumer that cannot do transparency, and such a consumer is
/// almost always compositing onto a white page. Discarding instead would keep
/// the colour underneath a fully transparent pixel, which in a straight-alpha
/// buffer is usually black — so a screenshot with rounded transparent corners
/// would paste into Paint with black corners rather than with none.
///
/// The `CF_DIBV5` published alongside it still carries the real alpha, and it
/// is first in the plan, so nothing that can see the alpha is made to look at
/// this one.
#[cfg(feature = "dib")]
fn encode_dib_legacy(item: &RichItem) -> FlavorResult {
    let RichItem::Image(Image::Rgba(img)) = item else {
        return none();
    };
    maybe(
        rclip_dib::encode_dib(
            img.width,
            img.height,
            &img.pixels,
            rclip_dib::Flatten::OVER_WHITE,
        )
        .ok(),
    )
}

#[cfg(not(feature = "dib"))]
fn encode_dib_legacy(_item: &RichItem) -> FlavorResult {
    Err("dib")
}

#[cfg(feature = "file-list")]
fn encode_file_list(item: &RichItem, platform: Platform) -> FlavorResult {
    let RichItem::Files(list) = item else {
        // A `Link` reaches `text/uri-list` through `Flavor::Url`, which is the
        // same MIME type on X11 and Wayland. Emitting it twice would put the
        // URL on the clipboard twice.
        return none();
    };

    match platform {
        Platform::Windows => {
            let mut builder = rclip_dropfiles::Builder::wide();
            for entry in &list.entries {
                // A path containing a NUL would truncate its own entry and
                // shift every path after it. Drop it rather than corrupt the
                // list.
                let _ = builder.push_str(entry.as_str());
            }
            one(builder.finish())
        }
        // One pasteboard *item* per file, which is the only way to say "these
        // four files" on a macOS pasteboard: an item offering `public.file-url`
        // four times is one file advertised four times, and
        // `-[NSPasteboard dataForType:]` would read back the first.
        Platform::MacOs => per_item(
            list.entries
                .iter()
                .map(|e| text::encode_plain(&to_uri(e), platform))
                .collect(),
        ),
        Platform::Unix => {
            let uris: Vec<String> = list.entries.iter().map(to_uri).collect();
            one(rclip_uri_list::emit::write_uri_list(
                uris.iter().map(String::as_str),
            ))
        }
    }
}

#[cfg(not(feature = "file-list"))]
fn encode_file_list(_item: &RichItem, _platform: Platform) -> FlavorResult {
    Err("file-list")
}

#[cfg(feature = "file-list")]
fn encode_convention(item: &RichItem, name: &str) -> FlavorResult {
    use rclip_uri_list::{convention, emit, FileAction};

    let RichItem::Files(list) = item else {
        return none();
    };
    let action = match list.action {
        TransferAction::Move => FileAction::Cut,
        // `DROPEFFECT_LINK` has no spelling in any of the Linux conventions,
        // and publishing it as a cut would move the user's files.
        TransferAction::Copy | TransferAction::Link => FileAction::Copy,
    };
    let uris: Vec<String> = list.entries.iter().map(to_uri).collect();
    match name {
        convention::MIME_GNOME_COPIED_FILES | convention::MIME_MATE_COPIED_FILES => one(
            emit::write_copied_files(action, uris.iter().map(String::as_str)),
        ),
        convention::MIME_KDE_CUT_SELECTION => one(Vec::from(emit::kde_cut_selection(action))),
        _ => none(),
    }
}

#[cfg(not(feature = "file-list"))]
fn encode_convention(_item: &RichItem, _name: &str) -> FlavorResult {
    Err("file-list")
}

#[cfg(feature = "file-desc")]
fn encode_file_desc(item: &RichItem) -> FlavorResult {
    let RichItem::PromisedFiles(files) = item else {
        return none();
    };
    let mut builder = rclip_file_desc::Builder::new();
    for file in files {
        let mut raw = rclip_file_desc::RawDescriptor::new();
        if let Some(size) = file.size {
            raw = raw.with_file_size(size);
        }
        if let Some(attrs) = file.attributes {
            raw = raw.with_attributes(attrs);
        }
        if let Some(t) = file.last_write_filetime {
            raw = raw.with_last_write_time(t);
        }
        // A name too long for `cFileName`, or one with a NUL in it, is dropped
        // rather than truncated: a truncated name is a different file.
        let _ = builder.push(raw, &file.name);
    }
    one(builder.finish())
}

#[cfg(not(feature = "file-desc"))]
fn encode_file_desc(_item: &RichItem) -> FlavorResult {
    Err("file-desc")
}

/// Turn a [`FileEntry`](crate::FileEntry) into the URI a `text/uri-list` wants.
///
/// A path becomes a percent-encoded `file://` URI through
/// [`rclip_uri_list::emit::file_uri`]; a URI is passed through verbatim,
/// because it was already encoded when it arrived and re-encoding would double
/// every `%`.
///
/// The encoding is `EncodeSet::Path` — RFC 3986 `pchar` plus `/` — which is
/// what makes a comparison on the receiving side work: §6.2.2.2 is explicit
/// that a percent-encoded reserved character is *not* equivalent to its literal
/// form, so a URI that escapes more than GTK does is a URI that stops matching
/// the ones every GTK application produces.
///
/// Measured against real GLib 2.88 rather than assumed: over every printable
/// ASCII byte and a range of multi-byte UTF-8, this agrees with
/// `g_filename_to_uri` on all of them but one. GLib escapes `;` as `%3B` and
/// this does not, because `g_filename_to_uri` does not in fact use
/// `G_URI_RESERVED_CHARS_ALLOWED_IN_PATH` — it escapes against an *unsafe* list
/// whose allowed reserved set is `:@&=+$,` plus `/`, and `;` is not in it. Both
/// spellings percent-decode to the same path, so nothing is lost or misread;
/// what differs is a textual comparison against a URI GTK minted itself, for a
/// filename containing a semicolon.
#[cfg(feature = "file-list")]
fn to_uri(entry: &crate::item::FileEntry) -> String {
    use crate::item::FileEntry;

    match entry {
        FileEntry::Uri(u) => u.clone(),
        FileEntry::Path(p) => rclip_uri_list::emit::file_uri(p),
    }
}

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
            Ok(blobs) => {
                for bytes in blobs {
                    payload.push(ClipboardItem::new(native.clone(), bytes));
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

/// The blobs to publish under one flavor.
///
/// Usually zero (the flavor does not apply to this item) or one. It is a `Vec`
/// because of macOS: a pasteboard models a multi-file selection as N *items*,
/// each carrying one `public.file-url`, and there is no other way to say "these
/// four files" there. `Err` names the Cargo feature whose codec was missing.
type FlavorResult = core::result::Result<Vec<Vec<u8>>, &'static str>;

fn none() -> FlavorResult {
    Ok(Vec::new())
}

/// Every caller is behind a format feature; a build with none of them on
/// publishes only plain text, which goes through [`maybe`].
#[cfg_attr(not(feature = "full"), allow(dead_code))]
fn one(bytes: Vec<u8>) -> FlavorResult {
    Ok(alloc::vec![bytes])
}

fn maybe(bytes: Option<Vec<u8>>) -> FlavorResult {
    Ok(bytes.into_iter().collect())
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

        Flavor::DibV5 => encode_dib(item, options),
        Flavor::Png => maybe(encoded_image_bytes(item, ImageFormat::Png)),
        Flavor::Jpeg => maybe(encoded_image_bytes(item, ImageFormat::Jpeg)),
        Flavor::Gif => maybe(encoded_image_bytes(item, ImageFormat::Gif)),
        Flavor::Tiff => maybe(encoded_image_bytes(item, ImageFormat::Tiff)),
        // `rclip-dib`'s encoder emits exactly one shape — 32-bpp BI_BITFIELDS
        // BITMAPV5HEADER — because a producer has no reason to write a format
        // that cannot carry alpha. So the `CF_DIB` line of the Windows plan is
        // advertised and never filled.
        //
        // TODO(phase-5): a `CF_DIB` (BITMAPINFOHEADER) encoder in `rclip-dib`,
        // for the Win32 applications that read `CF_DIB` and not `CF_DIBV5`.
        Flavor::Dib => none(),

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

fn encoded_image_bytes(item: &RichItem, want: ImageFormat) -> Option<Vec<u8>> {
    match item {
        RichItem::Image(Image::Encoded { format, bytes }) if *format == want => Some(bytes.clone()),
        // A decoded image cannot be re-encoded here: `plan/PLAN.md` §4.4 keeps
        // PNG, JPEG, GIF and TIFF out of this workspace on purpose, so there is
        // no encoder to call. A consumer that has `image` should encode first
        // and hand over an `Image::Encoded`.
        //
        // TODO(phase-5): an optional `image` feature on this crate that closes
        // the loop, so `Image::Rgba` can be published on macOS at all — the
        // plan there offers `public.png` and `public.tiff`, and this crate can
        // produce neither from pixels.
        _ => None,
    }
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
fn encode_dib(item: &RichItem, options: &Options) -> FlavorResult {
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
fn encode_dib(_item: &RichItem, _options: &Options) -> FlavorResult {
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
        // One pasteboard item per file. `ClipboardPayload` is a flat list, so
        // they come out as repeated `public.file-url` entries and the transport
        // groups them.
        Platform::MacOs => Ok(list
            .entries
            .iter()
            .map(|e| text::encode_plain(&to_uri(e), platform))
            .collect()),
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
/// A path becomes a percent-encoded `file://` URI; a URI is passed through
/// verbatim, because it was already encoded when it arrived and re-encoding
/// would double every `%`.
///
/// The percent-*encoder* lives here because `rclip-uri-list` has a decoder and
/// no encoder — its `emit` module writes URIs through verbatim and says so:
/// "percent-encoding them is the caller's job".
///
/// `// TODO(phase-5):` move this into `rclip-uri-list` next to
/// `Uri::percent_decode`, which is its mirror.
#[cfg(feature = "file-list")]
fn to_uri(entry: &crate::item::FileEntry) -> String {
    use crate::item::FileEntry;

    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    match entry {
        FileEntry::Uri(u) => u.clone(),
        FileEntry::Path(p) => {
            let mut out = String::from("file://");
            for &b in p.as_bytes() {
                match b {
                    // RFC 3986 unreserved, plus the separators a path is made
                    // of. `%` is deliberately *not* in the set: a literal `%`
                    // in a filename must become `%25` or the reader takes it
                    // for an escape.
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(b as char),
                    b'-' | b'.' | b'_' | b'~' | b'/' | b':' => out.push(b as char),
                    other => {
                        out.push('%');
                        out.push(HEX[usize::from(other >> 4)] as char);
                        out.push(HEX[usize::from(other & 0x0F)] as char);
                    }
                }
            }
            out
        }
    }
}

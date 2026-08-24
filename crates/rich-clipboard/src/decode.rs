//! Bytes to [`RichItem`].

use alloc::string::String;
use alloc::vec::Vec;

use rclip_core::{
    ClipboardItem, ClipboardPayload, Flavor, Limits, Oversize, OversizePolicy, Platform, SizeHint,
};

use crate::error::{Error, Result};
use crate::item::{Image, ImageFormat, RichItem, TransferAction};
use crate::text;

/// Decoding policy the payload cannot supply for itself.
///
/// Two knobs: the `CF_DIBV5` alpha mode, which `plan/PLAN.md` §4.4 says must
/// never be a silent guess, and whether an HTML flavor comes back as styled
/// runs or as the markup it arrived as.
#[derive(Debug, Clone)]
// Only `alpha` has a default that is not the derived one, so only a build with
// `dib` needs the impl written out; anywhere else a hand-written one is exactly
// what `clippy::derivable_impls` objects to.
#[cfg_attr(not(feature = "dib"), derive(Default))]
#[non_exhaustive]
pub struct Options {
    #[cfg(feature = "dib")]
    alpha: rclip_dib::AlphaMode,
    #[cfg(feature = "html")]
    keep_html_markup: bool,
    limits: Limits,
}

#[cfg(feature = "dib")]
impl Default for Options {
    fn default() -> Self {
        Self {
            // `Guess` rather than `Straight`, because the two producers a
            // desktop paste is most likely to come from — Chromium and Firefox
            // — both write premultiplied, and the heuristic is one-directional
            // in the safe direction: it only ever *proves* straight.
            #[cfg(feature = "dib")]
            alpha: rclip_dib::AlphaMode::Guess,
            #[cfg(feature = "html")]
            keep_html_markup: false,
            limits: Limits::default(),
        }
    }
}

impl Options {
    /// The defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Caps for this decode.
    ///
    /// These are a *second* line of defence, and it matters which. By the time
    /// a [`ClipboardPayload`] exists the bytes are already resident, because
    /// something read them — so this cannot stop a huge payload arriving. What
    /// it stops is *decoding* one, which is where the amplification lives: a
    /// 60 MB 8-bit `CF_DIB` becomes 240 MB of RGBA, and a flavor refused here
    /// falls through to the next-best one, so a paste that would have cost a
    /// quarter of a gigabyte becomes the plain-text version of the same thing.
    ///
    /// The first line belongs to the transport, before it reads, using
    /// [`SizeHint`]. See `plan/PLAN.md` §4b.
    #[must_use]
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The caps in force.
    #[must_use]
    pub const fn limits_ref(&self) -> &Limits {
        &self.limits
    }

    /// How to interpret the alpha channel of a `CF_DIBV5`.
    ///
    /// There is no in-band signal: Chromium and Firefox write premultiplied
    /// RGBA, XnView and Photoshop read the same bytes as straight. The default
    /// is `AlphaMode::Guess`, which is a documented heuristic and not a
    /// detector — pass an explicit mode whenever the source application is
    /// known.
    #[cfg(feature = "dib")]
    #[must_use]
    pub fn alpha(mut self, mode: rclip_dib::AlphaMode) -> Self {
        self.alpha = mode;
        self
    }

    /// The alpha mode, for the encoder.
    #[cfg(feature = "dib")]
    pub(crate) fn wire_alpha(&self) -> rclip_dib::AlphaMode {
        self.alpha
    }

    /// Hand back an HTML flavor as markup rather than as styled runs.
    ///
    /// By default an HTML flavor decodes to [`RichItem::RichText`]: the
    /// fragment is tokenized and its inline styling becomes runs, which is what
    /// an application that pastes *content* wants and what closes the last leg
    /// of the [`RichText`](crate::RichText) hub.
    ///
    /// Turn this on and it decodes to [`RichItem::Html`] instead, markup
    /// intact. That is what a clipboard bridge, an inspector, or anything that
    /// will re-publish the payload wants, because the conversion to runs is
    /// lossy in ways the markup is not: links, images and table structure have
    /// nowhere to go in a `RichText`, and `SourceURL` and the surrounding
    /// context document are carried only by [`HtmlFragment`](crate::HtmlFragment).
    #[cfg(feature = "html")]
    #[must_use]
    pub fn keep_html_markup(mut self, keep: bool) -> Self {
        self.keep_html_markup = keep;
        self
    }
}

/// Decode one clipboard item.
///
/// `platform` says which vocabulary [`ClipboardItem::native`] is written in —
/// `"HTML Format"`, `"public.html"` and `"text/html"` are the same flavor and
/// only the caller knows which world the bytes came from.
///
/// # Errors
///
/// [`Error::FeatureDisabled`] when the workspace has a codec for the flavor but
/// this build does not, [`Error::Codec`] when the bytes are malformed,
/// [`Error::NotContent`] for a metadata-only flavor, and [`Error::Unsupported`]
/// for a flavor nothing decodes. An *unrecognised* identifier is not an error:
/// it decodes to [`RichItem::Unknown`].
pub fn decode(item: &ClipboardItem, platform: Platform) -> Result<RichItem> {
    decode_with(item, platform, &Options::default())
}

/// Decode one clipboard item, with explicit policy.
///
/// # Errors
///
/// As [`decode`].
#[allow(unused_variables)]
pub fn decode_with(
    item: &ClipboardItem,
    platform: Platform,
    options: &Options,
) -> Result<RichItem> {
    let native = item.native.as_str();
    let bytes = item.bytes.as_slice();
    match item.flavor(platform) {
        Flavor::PlainText => Ok(RichItem::Text(text::decode_plain(bytes, platform))),

        Flavor::Html => decode_html(native, bytes, platform, options),
        Flavor::Rtf => decode_rtf(native, bytes),

        Flavor::Png => Ok(encoded_image(ImageFormat::Png, bytes)),
        Flavor::Jpeg => Ok(encoded_image(ImageFormat::Jpeg, bytes)),
        Flavor::Gif => Ok(encoded_image(ImageFormat::Gif, bytes)),
        Flavor::Tiff => Ok(encoded_image(ImageFormat::Tiff, bytes)),
        Flavor::Dib | Flavor::DibV5 => decode_dib(native, bytes, options),

        Flavor::FileList => decode_file_list(native, bytes, platform),
        Flavor::ShellIdList => decode_id_list(native, bytes),
        Flavor::FileDescriptor => decode_file_desc(native, bytes),
        Flavor::ShellLink => decode_shell_link(native, bytes),

        Flavor::Url => Ok(RichItem::Link(crate::Link::to_url(text::decode_plain(
            bytes, platform,
        )))),
        // `public.url-name` and `Preferred DropEffect` describe a sibling
        // flavor. `decode_payload` folds them in; on their own they are not an
        // item.
        Flavor::UrlName | Flavor::DropEffect => Err(Error::NotContent {
            native: String::from(native),
        }),
        // The descriptor names it and the transport streams it. There is no
        // byte layout here to parse.
        Flavor::FileContents => Err(Error::Unsupported {
            native: String::from(native),
        }),

        Flavor::Other(name) => decode_other(name, bytes),

        // `Flavor` is `#[non_exhaustive]`: a variant added to `rclip-core`
        // after this build must not stop the paste.
        _ => Ok(unknown(native, bytes)),
    }
}

/// Decode the richest flavor the payload offers that this build understands.
///
/// Candidates are tried in [`Flavor::read_rank`] order — richest first, because
/// plain text is derivable from rich text and never the reverse — and the first
/// one that decodes wins. A flavor whose feature is off is skipped rather than
/// fatal, which is the point of ranking rather than switching: a build without
/// `rtf` still gets styled text from `CF_HTML`, and a build with neither still
/// gets the characters.
///
/// The result is then enriched from the metadata flavors: a `Preferred
/// DropEffect` or a KDE cut flag becomes [`FileList::action`](crate::FileList),
/// a `public.url-name` becomes [`Link::title`](crate::Link), and a plain-text
/// sibling becomes [`HtmlFragment::plain`](crate::HtmlFragment::plain). Those are exactly the pieces that a
/// per-item decode cannot see and that go missing when a consumer decodes one
/// flavor and forgets the rest.
///
/// # Errors
///
/// [`Error::EmptyPayload`] for a payload with no items. Otherwise the first
/// error from the highest-ranked candidate that actually failed — a codec error
/// in preference to a disabled feature, because malformed bytes are the more
/// urgent thing to hear about. A payload whose flavors are merely
/// *unrecognised* is not an error: it decodes to [`RichItem::Unknown`].
pub fn decode_payload(payload: &ClipboardPayload) -> Result<RichItem> {
    decode_payload_with(payload, &Options::default())
}

/// Decode the richest flavor a payload offers, with explicit policy.
///
/// # Errors
///
/// As [`decode_payload`].
pub fn decode_payload_with(payload: &ClipboardPayload, options: &Options) -> Result<RichItem> {
    // The default policy skips an oversize flavor, which is why this can be the
    // no-callback entry point: skipping still yields a usable paste.
    decode_payload_policy(payload, options, &mut DefaultPolicy)
}

/// A policy that skips anything oversize, per [`OversizePolicy`]'s default.
struct DefaultPolicy;
impl OversizePolicy for DefaultPolicy {}

/// [`decode_payload_with`], asking `policy` what to do about an oversize flavor.
///
/// The callback fires *before* the bytes are decoded, once per flavor that
/// exceeds [`Limits::max_flavor_bytes`], and its answer decides that flavor
/// only:
///
/// - [`Oversize::Accept`] decodes it anyway.
/// - [`Oversize::Skip`] moves on to the next-best flavor. A 400 MB TIFF goes
///   and the plain text stays, which is why it is the default.
/// - [`Oversize::Abort`] fails the whole paste with [`Error::Oversize`].
///
/// A closure is a policy. Its parameters need annotating, because `Flavor`
/// borrows and inference will not produce the higher-ranked bound on its own:
///
/// ```ignore
/// let item = decode_payload_policy(
///     &payload,
///     &opts,
///     &mut |flavor: Flavor<'_>, hint: SizeHint, _: &Limits| {
///         log::warn!("skipping {flavor:?} at {hint:?}");
///         Oversize::Skip
///     },
/// )?;
/// ```
///
/// # Errors
///
/// As [`decode_payload_with`], plus [`Error::Oversize`] if every flavor was
/// refused on size.
pub fn decode_payload_policy(
    payload: &ClipboardPayload,
    options: &Options,
    policy: &mut impl OversizePolicy,
) -> Result<RichItem> {
    let platform = payload.platform();
    if payload.is_empty() {
        return Err(Error::EmptyPayload);
    }

    let mut candidates: Vec<&ClipboardItem> = payload
        .items()
        .iter()
        .filter(|i| i.flavor(platform).is_content())
        .collect();
    // A stable sort, so a source that offered the same flavor twice keeps its
    // own ordering — the first listing is the one it meant.
    candidates.sort_by_key(|i| i.flavor(platform).read_rank());

    let limits = &options.limits;
    let mut first_error: Option<Error> = None;
    // Remembered so that a payload where *everything* was refused reports the
    // largest offender rather than the last one looked at, which is the more
    // useful number when someone asks why their paste did nothing.
    let mut biggest_refused: Option<(&'static str, u64)> = None;

    for item in candidates {
        let flavor = item.flavor(platform);
        // The size is exact here: the bytes are in hand. A transport that has
        // not read yet gets `AtLeast` or `Unknown` instead, which is the whole
        // reason `SizeHint` is three-valued.
        let hint = SizeHint::Exact(item.bytes.len() as u64);
        if limits.rejects(hint) {
            match policy.on_oversize(flavor, hint, limits) {
                Oversize::Accept => {}
                Oversize::Skip => {
                    let name = flavor_name(flavor);
                    let bytes = item.bytes.len() as u64;
                    if biggest_refused.is_none_or(|(_, b)| bytes > b) {
                        biggest_refused = Some((name, bytes));
                    }
                    continue;
                }
                Oversize::Abort => {
                    return Err(Error::Oversize {
                        flavor: flavor_name(flavor),
                        bytes: item.bytes.len() as u64,
                        limit: limits.max_flavor_bytes,
                    })
                }
            }
        }
        match decode_with(item, platform, options) {
            Ok(decoded) => return Ok(enrich(decoded, payload, options)),
            Err(e) => {
                // A malformed payload is worth reporting over a feature that
                // happens to be off, so a codec error displaces a
                // FeatureDisabled that was recorded earlier.
                let better = matches!(e, Error::Codec { .. })
                    && !matches!(first_error, Some(Error::Codec { .. }));
                if first_error.is_none() || better {
                    first_error = Some(e);
                }
            }
        }
    }

    if let Some(e) = first_error {
        return Err(e);
    }
    // Nothing decoded and nothing failed: either every flavor was refused on
    // size, or every one was unrecognised.
    if let Some((flavor, bytes)) = biggest_refused {
        return Err(Error::Oversize {
            flavor,
            bytes,
            limit: limits.max_flavor_bytes,
        });
    }
    // Every content flavor was unrecognised, so `decode` returned
    // `RichItem::Unknown` for each and none of them failed. Unreachable in
    // practice; kept as the honest fallback rather than an `unwrap`.
    Ok(unknown_from_payload(payload))
}

/// A `'static` name for a flavor, for [`Error::Oversize`].
///
/// `Flavor::Other` borrows, so it cannot be named here; every other variant is
/// a unit and its `Debug` spelling is stable.
fn flavor_name(flavor: Flavor<'_>) -> &'static str {
    match flavor {
        Flavor::PlainText => "PlainText",
        Flavor::PlainTextUtf16 => "PlainTextUtf16",
        Flavor::Html => "Html",
        Flavor::Rtf => "Rtf",
        Flavor::WebArchive => "WebArchive",
        Flavor::Png => "Png",
        Flavor::Jpeg => "Jpeg",
        Flavor::Gif => "Gif",
        Flavor::Tiff => "Tiff",
        Flavor::Dib => "Dib",
        Flavor::DibV5 => "DibV5",
        Flavor::FileList => "FileList",
        Flavor::ShellIdList => "ShellIdList",
        Flavor::FileDescriptor => "FileDescriptor",
        Flavor::FileContents => "FileContents",
        Flavor::Url => "Url",
        Flavor::UrlName => "UrlName",
        Flavor::ShellLink => "ShellLink",
        Flavor::DropEffect => "DropEffect",
        _ => "Other",
    }
}

/// Decode every content flavor the payload offers, richest first.
///
/// For a caller that wants to see all of them — a clipboard inspector, or an
/// application choosing by its own rules rather than by `read_rank`. Flavors
/// that fail to decode are dropped; use [`decode`] per item to see why.
#[must_use]
pub fn decode_all(payload: &ClipboardPayload) -> Vec<RichItem> {
    let platform = payload.platform();
    let mut items: Vec<&ClipboardItem> = payload
        .items()
        .iter()
        .filter(|i| i.flavor(platform).is_content())
        .collect();
    items.sort_by_key(|i| i.flavor(platform).read_rank());
    items
        .into_iter()
        .filter_map(|i| decode(i, platform).ok())
        .collect()
}

/// Read the cut-vs-copy verb out of a payload, wherever it hid it.
///
/// Windows puts it in a `Preferred DropEffect` DWORD; GNOME puts it in the
/// first line of `x-special/gnome-copied-files`; KDE puts it in a one-byte
/// `application/x-kde-cutselection`. macOS does not have it at all, because the
/// Finder has no cut.
///
/// [`TransferAction::Copy`] when nothing says otherwise — the safe reading,
/// since guessing "move" would delete a user's files.
#[must_use]
pub fn transfer_action(payload: &ClipboardPayload) -> TransferAction {
    let platform = payload.platform();
    for item in payload.items() {
        match item.flavor(platform) {
            Flavor::DropEffect => {
                if let Some(action) = drop_effect(&item.bytes) {
                    return action;
                }
            }
            Flavor::Other("application/x-kde-cutselection") => {
                // `KIO::isClipboardDataCut` looks at byte zero and nothing
                // else, so this does too.
                if item.bytes.first() == Some(&b'1') {
                    return TransferAction::Move;
                }
            }
            Flavor::Other("x-special/gnome-copied-files" | "x-special/mate-copied-files") => {
                if let Some(action) = gnome_verb(&item.bytes) {
                    return action;
                }
            }
            _ => {}
        }
    }
    TransferAction::Copy
}

/// `DROPEFFECT_*` bits. `MOVE` wins a `COPY | MOVE` combination, because a
/// source that set both is offering a choice and the paste target asked for the
/// preferred one.
fn drop_effect(bytes: &[u8]) -> Option<TransferAction> {
    use rclip_core::flavor::drop_effect as fx;
    let word = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    if word & fx::MOVE != 0 {
        Some(TransferAction::Move)
    } else if word & fx::COPY != 0 {
        Some(TransferAction::Copy)
    } else if word & fx::LINK != 0 {
        Some(TransferAction::Link)
    } else {
        None
    }
}

fn gnome_verb(bytes: &[u8]) -> Option<TransferAction> {
    let line = bytes.split(|b| *b == b'\n').next()?;
    let line = core::str::from_utf8(line).ok()?.trim();
    if line.eq_ignore_ascii_case("cut") {
        Some(TransferAction::Move)
    } else if line.eq_ignore_ascii_case("copy") {
        Some(TransferAction::Copy)
    } else {
        None
    }
}

/// Fold the payload's metadata flavors into a decoded item.
fn enrich(item: RichItem, payload: &ClipboardPayload, _options: &Options) -> RichItem {
    let platform = payload.platform();
    match item {
        RichItem::Files(mut list) => {
            list.action = transfer_action(payload);
            // A macOS pasteboard models a multi-file selection as N *items*,
            // each offering one `public.file-url` — copying three files in
            // Finder produces three items, not one item offering the type
            // three times. `decode` is handed a single `ClipboardItem` and can
            // only ever see one file, so reassembling the selection is
            // `decode_payload`'s job and nothing else's.
            //
            // `ClipboardPayload::all` rather than `get`, which is the whole
            // point: first-match-wins is what turns a three-file paste into a
            // one-file paste.
            //
            // macOS only, and by platform rather than by
            // `ClipboardPayload::item_count`: `public.file-url` is one URI per
            // representation, so collecting them is addition. A `CF_HDROP` or a
            // `text/uri-list` already holds the whole selection in one blob, and
            // re-reading it as a single URI would replace the list with one
            // nonsense entry.
            if platform == Platform::MacOs {
                let mut parts: Vec<&ClipboardItem> = payload.all(Flavor::FileList).collect();
                // By item index, so a transport that enumerated the pasteboard
                // out of order still hands back the user's selection in the
                // order they made it. Stable, so two representations within one
                // item keep the source's ordering.
                parts.sort_by_key(|i| i.item);
                let all: Vec<_> = parts
                    .into_iter()
                    .filter_map(|i| file_entry_from_uri(&text::decode_plain(&i.bytes, platform)))
                    .collect();
                // Never fewer than the per-item decode already found: a URI
                // this build declines to turn into an entry must not take the
                // selection down with it.
                if all.len() > list.entries.len() {
                    list.entries = all;
                }
            }
            RichItem::Files(list)
        }
        RichItem::Link(mut link) => {
            if link.title.is_none() {
                link.title = payload
                    .get(Flavor::UrlName)
                    .map(|i| text::decode_plain(&i.bytes, platform));
            }
            RichItem::Link(link)
        }
        RichItem::Html(mut html) => {
            if html.plain.is_none() {
                html.plain = payload
                    .get(Flavor::PlainText)
                    .map(|i| text::decode_plain(&i.bytes, platform));
            }
            RichItem::Html(html)
        }
        other => other,
    }
}

fn unknown(native: &str, bytes: &[u8]) -> RichItem {
    RichItem::Unknown {
        native: String::from(native),
        bytes: Vec::from(bytes),
    }
}

fn unknown_from_payload(payload: &ClipboardPayload) -> RichItem {
    let item = payload
        .best()
        .or_else(|| payload.items().first())
        .expect("payload was checked non-empty");
    unknown(&item.native, &item.bytes)
}

fn encoded_image(format: ImageFormat, bytes: &[u8]) -> RichItem {
    RichItem::Image(Image::Encoded {
        format,
        bytes: Vec::from(bytes),
    })
}

// ---------------------------------------------------------------------------
// Per-flavor decoders, each present only when its feature is on
// ---------------------------------------------------------------------------

#[cfg(feature = "html")]
fn decode_html(
    native: &str,
    bytes: &[u8],
    platform: Platform,
    options: &Options,
) -> Result<RichItem> {
    use crate::item::HtmlFragment;

    let mut fragment = if platform == Platform::Windows {
        let parsed = rclip_cf_html::parse(bytes).map_err(|e| Error::codec(native, e))?;
        HtmlFragment {
            markup: String::from(parsed.fragment),
            context: parsed.context.map(String::from),
            source_url: parsed.source_url.map(String::from),
            plain: None,
        }
    } else {
        // `public.html` and `text/html` are bare markup with no header — and an
        // unreliable encoding, which `text::decode_html_bytes` sniffs.
        HtmlFragment {
            markup: text::decode_html_bytes(bytes),
            context: None,
            source_url: None,
            plain: None,
        }
    };

    if options.keep_html_markup {
        // The markup is what was asked for, but the plain text no longer has to
        // be guessed at: there is a tokenizer now, so `RichItem::plain_text`
        // can answer for an HTML item instead of returning `None`.
        fragment.plain = fragment.to_rich_text().ok().map(|t| t.text);
        return Ok(RichItem::Html(fragment));
    }
    fragment
        .to_rich_text()
        .map(RichItem::RichText)
        .map_err(|e| Error::codec(native, e))
}

#[cfg(not(feature = "html"))]
fn decode_html(
    _native: &str,
    _bytes: &[u8],
    _platform: Platform,
    _options: &Options,
) -> Result<RichItem> {
    Err(Error::FeatureDisabled {
        flavor: "Html",
        feature: "html",
    })
}

#[cfg(feature = "rtf")]
fn decode_rtf(native: &str, bytes: &[u8]) -> Result<RichItem> {
    crate::RichText::from_rtf(bytes)
        .map(RichItem::RichText)
        .map_err(|e| Error::codec(native, e))
}

#[cfg(not(feature = "rtf"))]
fn decode_rtf(_native: &str, _bytes: &[u8]) -> Result<RichItem> {
    Err(Error::FeatureDisabled {
        flavor: "Rtf",
        feature: "rtf",
    })
}

#[cfg(feature = "dib")]
fn decode_dib(native: &str, bytes: &[u8], options: &Options) -> Result<RichItem> {
    let img = rclip_dib::decode(bytes, options.alpha).map_err(|e| Error::codec(native, e))?;
    Ok(RichItem::Image(Image::Rgba(crate::RgbaImage {
        width: img.width,
        height: img.height,
        pixels: img.pixels,
    })))
}

#[cfg(not(feature = "dib"))]
fn decode_dib(_native: &str, _bytes: &[u8], _options: &Options) -> Result<RichItem> {
    Err(Error::FeatureDisabled {
        flavor: "Dib",
        feature: "dib",
    })
}

#[cfg(feature = "file-list")]
fn decode_file_list(native: &str, bytes: &[u8], platform: Platform) -> Result<RichItem> {
    use crate::item::{FileEntry, FileList};

    let entries = match platform {
        Platform::Windows => {
            let drop =
                rclip_dropfiles::DropFiles::parse(bytes).map_err(|e| Error::codec(native, e))?;
            drop.paths()
                // An ANSI path is bytes in the *source machine's* code page,
                // which is not in the payload. `rclip-dropfiles` refuses to
                // guess and so does this: a mangled path is worse than a
                // missing one, because a mangled one gets opened.
                .filter_map(|p| p.to_string_lossy())
                .map(FileEntry::Path)
                .collect()
        }
        // One `public.file-url` per item; `enrich` collects the siblings.
        Platform::MacOs => file_entry_from_uri(&text::decode_plain(bytes, platform))
            .into_iter()
            .collect(),
        Platform::Unix => {
            let list = rclip_uri_list::parse(bytes).map_err(|e| Error::codec(native, e))?;
            list.uris()
                .filter_map(|u| file_entry_from_uri(u.as_str()))
                .collect()
        }
    };
    Ok(RichItem::Files(FileList {
        entries,
        action: TransferAction::Copy,
    }))
}

#[cfg(not(feature = "file-list"))]
fn decode_file_list(_native: &str, _bytes: &[u8], _platform: Platform) -> Result<RichItem> {
    Err(Error::FeatureDisabled {
        flavor: "FileList",
        feature: "file-list",
    })
}

/// Turn one URI into a [`FileEntry`](crate::FileEntry).
///
/// A `file://` URI with an empty or `localhost` host becomes a percent-decoded
/// path; anything else stays a URI, because GNOME will happily copy something
/// out of an `sftp://` mount and flattening that to a path would name a file
/// that is not there.
fn file_entry_from_uri(uri: &str) -> Option<crate::item::FileEntry> {
    use crate::item::FileEntry;

    let uri = uri.trim();
    if uri.is_empty() {
        return None;
    }
    let Some(rest) = uri.strip_prefix("file://") else {
        return Some(FileEntry::Uri(String::from(uri)));
    };
    let (host, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        // `file:` with no path at all.
        None => return Some(FileEntry::Uri(String::from(uri))),
    };
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
        return Some(FileEntry::Uri(String::from(uri)));
    }
    Some(FileEntry::Path(percent_decode(path)))
}

/// Percent-decode, lossily.
///
/// A `%` that is not followed by two hex digits is passed through as a literal
/// `%`, which is what every desktop file manager's reader does: real payloads
/// contain unencoded `%` in filenames and rejecting them would drop the file.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    text::decode_utf8_lossy(&out)
}

#[cfg(feature = "id-list")]
fn decode_id_list(native: &str, bytes: &[u8]) -> Result<RichItem> {
    use crate::item::ShellItems;

    let cida = rclip_idlist::Cida::parse(bytes).map_err(|e| Error::codec(native, e))?;
    let parent = cida
        .parent()
        .ok()
        .map(|list| rclip_idlist::display_path(list, "\\"))
        .filter(|s| !s.is_empty());
    let display_paths = cida
        .children()
        .filter_map(core::result::Result::ok)
        .map(|list| rclip_idlist::display_path(list, "\\"))
        .filter(|s| !s.is_empty())
        .collect();
    Ok(RichItem::ShellItems(ShellItems {
        display_paths,
        parent,
    }))
}

#[cfg(not(feature = "id-list"))]
fn decode_id_list(_native: &str, _bytes: &[u8]) -> Result<RichItem> {
    Err(Error::FeatureDisabled {
        flavor: "ShellIdList",
        feature: "id-list",
    })
}

#[cfg(feature = "file-desc")]
fn decode_file_desc(native: &str, bytes: &[u8]) -> Result<RichItem> {
    use crate::item::PromisedFile;

    let group =
        rclip_file_desc::FileGroupDescriptor::parse(bytes).map_err(|e| Error::codec(native, e))?;
    let files = group
        .iter()
        .map(|d| PromisedFile {
            name: d.file_name_lossy(),
            size: d.file_size(),
            attributes: d.file_attributes(),
            last_write_filetime: d.last_write_time(),
            is_directory: d.is_directory(),
        })
        .collect();
    Ok(RichItem::PromisedFiles(files))
}

#[cfg(not(feature = "file-desc"))]
fn decode_file_desc(_native: &str, _bytes: &[u8]) -> Result<RichItem> {
    Err(Error::FeatureDisabled {
        flavor: "FileDescriptor",
        feature: "file-desc",
    })
}

#[cfg(feature = "shell-link")]
fn decode_shell_link(_native: &str, bytes: &[u8]) -> Result<RichItem> {
    crate::Shortcut::from_lnk(bytes).map(RichItem::Shortcut)
}

#[cfg(not(feature = "shell-link"))]
fn decode_shell_link(_native: &str, _bytes: &[u8]) -> Result<RichItem> {
    Err(Error::FeatureDisabled {
        flavor: "ShellLink",
        feature: "shell-link",
    })
}

/// Flavors `rclip-core`'s registry reports as [`Flavor::Other`] but that this
/// crate still knows something about.
///
/// The GNOME and MATE payloads are the only place a Linux file *list* and its
/// cut-vs-copy verb travel together, so they are worth decoding as content
/// rather than passing through as bytes.
#[allow(unused_variables)]
fn decode_other(name: &str, bytes: &[u8]) -> Result<RichItem> {
    #[cfg(feature = "file-list")]
    if matches!(
        name,
        "x-special/gnome-copied-files" | "x-special/mate-copied-files"
    ) {
        use crate::item::FileList;

        let parsed = rclip_uri_list::convention::parse_copied_files(bytes)
            .map_err(|e| Error::codec(name, e))?;
        let action = match parsed.action() {
            rclip_uri_list::FileAction::Cut => TransferAction::Move,
            _ => TransferAction::Copy,
        };
        return Ok(RichItem::Files(FileList {
            entries: parsed
                .uris()
                .filter_map(|u| file_entry_from_uri(u.as_str()))
                .collect(),
            action,
        }));
    }
    Ok(unknown(name, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_uri_becomes_a_decoded_path() {
        let entry = file_entry_from_uri("file:///home/me/a%20file.txt").unwrap();
        assert_eq!(entry.as_path(), Some("/home/me/a file.txt"));
    }

    #[test]
    fn a_remote_uri_stays_a_uri() {
        let entry = file_entry_from_uri("sftp://host/x").unwrap();
        assert_eq!(entry.as_path(), None);
    }

    #[test]
    fn a_stray_percent_survives_decoding() {
        assert_eq!(percent_decode("/100%25/50% off"), "/100%/50% off");
    }

    #[test]
    fn move_wins_a_combined_drop_effect() {
        assert_eq!(drop_effect(&3u32.to_le_bytes()), Some(TransferAction::Move));
    }

    #[test]
    fn a_short_drop_effect_word_is_no_answer_rather_than_a_wrong_one() {
        assert_eq!(drop_effect(&[2, 0]), None);
    }
}

//! The facade's read side — `rich_clipboard::decode`, `decode_all`,
//! `decode_payload`, `decode_payload_with` and `transfer_action`.
//!
//! This is the entry point a consumer actually calls, and until now nothing
//! fuzzed it. Everything under it was covered one codec at a time; what was
//! not covered is the *policy* layer that picks among them — the ranking, the
//! candidate loop, the error preference, the metadata folding, and the
//! multi-item grouping that `ClipboardPayload` gained in phase 5.
//!
//! # Why this target needs `arbitrary`
//!
//! `decode_payload` does not take bytes. It takes a `ClipboardPayload`: a
//! `Platform` plus a list of `(native identifier, bytes, item index)`. A
//! `&[u8]` target would have to invent that structure, and the obvious
//! inventions are all wrong — hand the whole buffer over as one item under one
//! fixed identifier and the mutator can never reach a second decoder; split it
//! on a separator byte and the payloads can never contain that byte.
//!
//! So the input is a structure, and the shape of that structure is the design
//! decision this target is really making:
//!
//! * **The platform is drawn from all three**, because the *same* identifier
//!   resolves differently in each vocabulary and the platform also changes what
//!   the decoders do — `Flavor::Html` is a `CF_HTML` header on Windows and bare
//!   markup everywhere else, and `Flavor::FileList` is three different parsers.
//! * **The identifier is a weighted mix.** Roughly four in five are drawn from
//!   [`NATIVE`], which is every identifier `rclip-core`'s registry knows plus
//!   the conventions layered on top of it; the rest are arbitrary strings. Both
//!   halves matter. Without the table a mutator would essentially never spell
//!   `"UniformResourceLocatorW"` and the real decoders would go unreached;
//!   without the arbitrary strings the `Flavor::Other` path — which is most of
//!   what a real clipboard offers — would go unreached instead. The table spans
//!   all three vocabularies deliberately, so a Windows payload offering
//!   `"text/html"` exercises the mismatch as well.
//! * **Item indices come from `0..=3`**, small enough that several
//!   representations land in the same pasteboard item and the grouping is
//!   actually exercised. Drawing a `usize` would put every representation in an
//!   item of its own and `group()` would never see a second member.
//!
//! # The encoding is deliberately hand-computable
//!
//! Every control field is one `int_in_range` over a range narrower than 256, so
//! it costs exactly one byte, and the last item's payload is the whole
//! remaining buffer. That makes the header for a single-item payload five fixed
//! bytes, which is what lets `seed-corpus.sh` turn each `corpus/synthetic/`
//! fixture into a real seed — a `.rtf` fixture prefixed with five bytes is a
//! macOS pasteboard offering `public.rtf`, and the mutator starts from a
//! payload that decodes instead of from noise. Using `arbitrary_len` here would
//! have read the length from the *end* of the buffer and made that impossible.
#![no_main]

use arbitrary::{Arbitrary, Result, Unstructured};
use libfuzzer_sys::fuzz_target;

use rich_clipboard::{
    decode, decode_all, decode_payload, decode_payload_with, decode_with, transfer_action,
    ClipboardItem, ClipboardPayload, Error, Flavor, Options, Platform, RichItem,
};

/// Every platform-native identifier the registry knows, plus the conventions
/// layered on it, plus a few real-world spellings it deliberately does not map.
///
/// One string per line, and `seed-corpus.sh` parses this table to find the
/// index of the identifier it wants. Keep it one-per-line.
const NATIVE: &[&str] = &[
    // Windows, predefined formats by their constant's name.
    "CF_UNICODETEXT",
    "CF_TEXT",
    "CF_OEMTEXT",
    "CF_TIFF",
    "CF_DIB",
    "CF_DIBV5",
    "CF_HDROP",
    // Windows, registered formats.
    "HTML Format",
    "Rich Text Format",
    "Rich Text Format Without Objects",
    "PNG",
    "JFIF",
    "GIF",
    "Shell IDList Array",
    "FileGroupDescriptorW",
    "FileGroupDescriptor",
    "FileContents",
    "UniformResourceLocatorW",
    "UniformResourceLocator",
    "Preferred DropEffect",
    "Performed DropEffect",
    "FileNameW",
    "MountedVolume",
    "UntrustedDragDrop",
    // macOS UTIs and their legacy pasteboard-type twins.
    "public.utf8-plain-text",
    "public.plain-text",
    "NSStringPboardType",
    "public.utf16-external-plain-text",
    "NSUnicodePboardType",
    "com.apple.webarchive",
    "public.html",
    "Apple HTML pasteboard type",
    "public.rtf",
    "NeXT Rich Text Format v1.0 pasteboard type",
    "public.png",
    "Apple PNG pasteboard type",
    "public.jpeg",
    "com.compuserve.gif",
    "public.tiff",
    "public.file-url",
    "NSFilenamesPboardType",
    "public.url",
    "Apple URL pasteboard type",
    "public.url-name",
    // The OSType wrapper, which the registry decodes rather than passing on.
    "CorePasteboardFlavorType 0x75743136",
    "CorePasteboardFlavorType 0x52544620",
    "dyn.ah62d4rv4gu8y6y4grf0gn5xbrzw",
    // X11 targets and Wayland MIME types.
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
    "text/html",
    "text/rtf",
    "application/rtf",
    "text/richtext",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/tiff",
    "image/bmp",
    "text/uri-list",
    // The desktop conventions, which are `Flavor::Other` but which the facade
    // still decodes.
    "x-special/gnome-copied-files",
    "x-special/mate-copied-files",
    "x-special/nautilus-clipboard",
    "application/x-kde-cutselection",
    "application/x-kde4-urilist",
    "text/x-moz-url",
];

const PLATFORMS: [Platform; 3] = [Platform::Windows, Platform::MacOs, Platform::Unix];

/// A plausible-but-hostile clipboard read.
#[derive(Debug)]
struct FuzzPayload {
    payload: ClipboardPayload,
}

impl<'a> Arbitrary<'a> for FuzzPayload {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        // 1 byte: `delta` is 2, which fits in one byte, so `int_in_range`
        // consumes exactly one. The same reasoning holds for every draw below,
        // and it is what `seed-corpus.sh` relies on.
        let platform = PLATFORMS[usize::from(u.int_in_range(0u8..=2)?)];
        let count = u.int_in_range(1u8..=6)?;

        let mut payload = ClipboardPayload::new(platform);
        for i in 0..count {
            // 13 in 16 from the registry, 3 in 16 arbitrary. Enough arbitrary
            // strings that `Flavor::Other` and `RichItem::Unknown` are reached
            // often, few enough that the real decoders still get most of the
            // budget.
            let native = if u.int_in_range(0u8..=15)? < 13 {
                String::from(*u.choose(NATIVE)?)
            } else {
                let want = usize::from(u.int_in_range(0u8..=32)?);
                let take = want.min(u.len());
                String::from_utf8_lossy(u.bytes(take)?).into_owned()
            };

            // A small range on purpose: several representations have to land in
            // the same pasteboard item for `group()` to have anything to group,
            // and a full `usize` would give every representation its own item.
            let item = usize::from(u.int_in_range(0u8..=3)?);

            let bytes = if i + 1 == count {
                // The last item takes the rest, so a seed is a five-byte header
                // followed by a real fixture.
                let rest = u.len();
                u.bytes(rest)?
            } else {
                // Exactly two bytes, whatever the buffer holds, so the header
                // stays a fixed size.
                let want = usize::from(u.int_in_range(0u16..=u16::MAX)?);
                let take = want.min(u.len());
                u.bytes(take)?
            };
            payload.push(ClipboardItem::in_item(item, native, bytes));
        }
        Ok(Self { payload })
    }
}

/// The error variants [`decode`] and [`decode_payload`] document. Anything else
/// coming back from the read side is the read side reporting a write-side
/// failure, which a caller has no way to act on.
fn is_decode_error(e: &Error) -> bool {
    matches!(
        e,
        Error::Codec { .. }
            | Error::FeatureDisabled { .. }
            | Error::Unsupported { .. }
            | Error::NotContent { .. }
            | Error::EmptyPayload
    )
}

fuzz_target!(|input: FuzzPayload| {
    let payload = input.payload;
    let platform = payload.platform();

    // ---------------------------------------------------------------- core
    // `ClipboardPayload`'s own accessors, which phase 5 rewrote. `group` and
    // `all` are new code and `item_count` is what a transport sizes its loop
    // with, so an off-by-one here loses a file from a multi-file paste.
    let count = payload.item_count();
    assert_eq!(
        count == 0,
        payload.is_empty(),
        "item_count and is_empty disagreed"
    );
    if count > 0 {
        // `item_count` is the highest index plus one, so the highest index has
        // to exist. A transport sizes its read loop with this number, and one
        // too many is an empty pasteboard item handed to the application.
        assert!(
            payload.group(count - 1).next().is_some(),
            "item_count claimed an item with no representations"
        );
    }
    let mut grouped = 0usize;
    for i in 0..count {
        let members: Vec<_> = payload.group(i).collect();
        for m in &members {
            assert_eq!(m.item, i, "group() returned a member of another item");
        }
        grouped += members.len();
    }
    assert_eq!(
        grouped,
        payload.len(),
        "the groups did not partition the payload"
    );
    for flavor in [Flavor::FileList, Flavor::PlainText, Flavor::Html] {
        let n = payload.all(flavor).count();
        let expected = payload
            .items()
            .iter()
            .filter(|i| i.flavor(platform) == flavor)
            .count();
        assert_eq!(n, expected, "all() and a manual filter disagreed");
        if n > 0 {
            assert!(payload.get(flavor).is_some(), "all() found what get() did not");
        }
    }

    // ------------------------------------------------------------- per item
    let mut any_content_decoded = false;
    let mut has_content = false;
    for item in payload.items() {
        let flavor = item.flavor(platform);
        has_content |= flavor.is_content();

        match decode(item, platform) {
            Ok(decoded) => {
                any_content_decoded |= flavor.is_content();
                // An unrecognised identifier is documented as *not* an error:
                // it comes back verbatim so a bridge can republish it.
                if let RichItem::Unknown { native, bytes } = &decoded {
                    assert_eq!(native, &item.native, "Unknown lost the identifier");
                    assert_eq!(bytes, &item.bytes, "Unknown lost the bytes");
                }
                let _ = decoded.plain_text();
                let _ = decoded.kind();
            }
            Err(e) => {
                assert!(is_decode_error(&e), "decode returned a write-side error: {e:?}");
                // `EmptyPayload` is about a payload, not an item, so a
                // per-item decode can never produce it.
                assert!(
                    !matches!(e, Error::EmptyPayload),
                    "decode of one item reported an empty payload"
                );
                match &e {
                    // Documented as the metadata-only flavors and nothing else.
                    Error::NotContent { native } => {
                        assert_eq!(native, &item.native);
                        assert!(
                            !flavor.is_content(),
                            "a content flavor was reported as metadata: {flavor:?}"
                        );
                    }
                    // `FileContents` is streamed by the transport and has no
                    // byte layout to parse; nothing else reaches here, because
                    // an unknown identifier decodes to `Unknown` instead.
                    Error::Unsupported { native } => {
                        assert_eq!(native, &item.native);
                        assert_eq!(
                            flavor,
                            Flavor::FileContents,
                            "an unsupported flavor other than FileContents: {flavor:?}"
                        );
                    }
                    Error::Codec { native, source } => {
                        assert_eq!(native, &item.native);
                        // Deliberately *not* asserted: that `source.offset` is
                        // inside the item. It is not, and the reason is one
                        // level down -- `rclip_core::Reader::seek`, `slice_at`
                        // and `tail_at` all report the offset that was *asked
                        // for* rather than a position in the buffer, so a
                        // `CF_HDROP` whose `pFiles` says 65536 fails with
                        // `BadOffset` at 65536 on a 40-byte item. That is a
                        // useful diagnostic and a plausible design, but
                        // `rclip_core::Error::offset` is documented as "byte
                        // offset into the buffer handed to the parser", and a
                        // caller that believes the doc and slices `&bytes[..
                        // err.offset]` to show context panics.
                        // `regression-seek-offset-past-buffer.bin` is the
                        // 45-byte payload that shows it.
                        let _ = source.offset;
                    }
                    _ => {}
                }
            }
        }
    }

    // ------------------------------------------------------------- payload
    let all = decode_all(&payload);
    assert!(
        all.len() <= payload.len(),
        "decode_all produced more items than the payload had"
    );

    match decode_payload(&payload) {
        Ok(_) => {
            assert!(!payload.is_empty(), "an empty payload decoded to something");
            if has_content {
                // The two entry points run the same candidate loop over the
                // same set with the same options, so one finding something and
                // the other finding nothing means they disagree about what
                // "decodes" means.
                assert!(
                    !all.is_empty() || !any_content_decoded,
                    "decode_payload succeeded where decode_all found nothing"
                );
            }
        }
        Err(e) => {
            assert!(
                is_decode_error(&e),
                "decode_payload returned a write-side error: {e:?}"
            );
            assert_eq!(
                matches!(e, Error::EmptyPayload),
                payload.is_empty(),
                "EmptyPayload disagreed with is_empty()"
            );
            assert!(
                all.is_empty(),
                "decode_payload failed while decode_all decoded {} item(s)",
                all.len()
            );
        }
    }

    // Deterministic: the same bytes must decode to the same thing twice. A
    // difference would mean something in the policy layer reads state that is
    // not in the payload.
    assert!(
        decode_payload(&payload) == decode_payload(&payload),
        "decode_payload was not deterministic"
    );

    // --------------------------------------------------------- with options
    // `keep_html_markup` is a different branch of `decode_html`, not a flag on
    // the result: it fills `plain` by tokenizing, which is a second pass over
    // the same attacker-controlled markup.
    let markup = Options::new().keep_html_markup(true);
    match decode_payload_with(&payload, &markup) {
        Ok(RichItem::Html(fragment)) => {
            // `to_rich_text` is what filled `plain`; running it again must not
            // depend on having run it once.
            let _ = fragment.to_rich_text();
        }
        Ok(_) => {}
        Err(e) => assert!(is_decode_error(&e), "decode returned a write-side error: {e:?}"),
    }

    // Every alpha policy. `Guess` inspects every pixel of a `CF_DIBV5`, so it
    // is a different code path rather than a different constant, and it is the
    // default a consumer gets without asking.
    for alpha in [
        rclip_dib::AlphaMode::Straight,
        rclip_dib::AlphaMode::Premultiplied,
        rclip_dib::AlphaMode::Guess,
    ] {
        let options = Options::new().alpha(alpha);
        if let Err(e) = decode_payload_with(&payload, &options) {
            assert!(is_decode_error(&e), "decode returned a write-side error: {e:?}");
        }
        for item in payload.items() {
            if let Err(e) = decode_with(item, platform, &options) {
                assert!(is_decode_error(&e), "decode returned a write-side error: {e:?}");
            }
        }
    }

    // The cut-vs-copy verb, which is read from three different places
    // depending on the desktop and which decides whether a paste *moves* a
    // user's files. Defaulting to `Copy` when nothing says otherwise is the
    // safe reading and is the property worth pinning.
    let action = transfer_action(&payload);
    if payload
        .items()
        .iter()
        .all(|i| !matches!(i.flavor(platform), Flavor::DropEffect | Flavor::Other(_)))
    {
        assert_eq!(
            action,
            rich_clipboard::TransferAction::Copy,
            "a payload with no verb flavor was not read as a copy"
        );
    }
});

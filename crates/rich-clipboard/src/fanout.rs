//! The write-side fan-out table: which flavors to publish, and what each costs.
//!
//! [`rclip_core::Flavor::read_rank`] answers the *read* question — a source
//! offered five things, which one do I decode. This module answers the other
//! one, and the two are not the same question turned around.
//!
//! Reading is a choice between representations that already exist, so the
//! ranking can be a single total order: richer wins, because plain text is
//! derivable from rich text and never the reverse. Writing is not a choice at
//! all. The application on the other end of the paste decides what it takes,
//! and it is not going to tell you first, so the answer is *all of them* — every
//! flavor the item can be expressed in, published simultaneously, best first.
//! Paste styled text into Word and it wants `Rich Text Format`; into Chrome and
//! it wants `HTML Format`; into Notepad and it wants `CF_UNICODETEXT`. Offer one
//! and two of those three pastes are worse than they had to be.
//!
//! The two tables also disagree on substance, not just on shape. Reading prefers
//! `PNG` over `CF_DIBV5` because PNG has an unambiguous alpha convention and
//! `CF_DIBV5` famously does not. Writing on Windows leads with `CF_DIBV5`
//! anyway, because Paint, older Office and a long tail of Win32 applications
//! read `CF_DIB`/`CF_DIBV5` and nothing else — a PNG-only offer pastes as
//! nothing at all in Paint. Different questions, different answers, two tables.
//!
//! # Order
//!
//! Best first. On Windows the order is a *stated preference* more than a
//! mechanism: nearly every rich-text consumer asks for a specific format id
//! rather than taking the first one `EnumClipboardFormats` yields, so what
//! matters is the set. It still matters to `GetPriorityClipboardFormat`
//! callers, to the order a `TARGETS` reply lists on X11, and to a human reading
//! a `wl-paste --list-types` dump — so it is worth getting right, and it is
//! worth agreeing with `read_rank` wherever the two have no reason to differ.

use rclip_core::{Flavor, Platform};

/// What is being published, before it is spread across flavors.
///
/// The key into [`write_plan`]. Obtained from
/// [`RichItem::kind`](crate::RichItem::kind), or named directly by a caller
/// that wants to inspect the table without holding an item.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ItemKind {
    /// Unstyled UTF-8 text.
    Text,
    /// Styled text, in the [`RichText`](crate::RichText) hub representation.
    RichText,
    /// An HTML fragment this crate did not author and cannot decompose.
    Html,
    /// A raster image.
    Image,
    /// A list of files that exist.
    Files,
    /// Descriptors for files that do not exist yet.
    PromisedFiles,
    /// Something that points somewhere: a URL, or a shortcut file's target.
    Link,
    /// A parsed `.lnk`.
    Shortcut,
    /// Windows shell namespace objects.
    ShellItems,
    /// A flavor this build could not decode, carried through verbatim.
    Unknown,
}

/// What a flavor carries, relative to the item it was made from.
///
/// The lossiness of a conversion is a property a caller has to be able to act
/// on — a clipboard bridge deciding whether a round-trip is safe, a "paste
/// special" menu deciding what to list — so it is in the type system rather
/// than only in prose.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Fidelity {
    /// Everything the item holds survives into these bytes.
    Full,
    /// Something is dropped. [`WriteFlavor::note`] says what.
    Lossy,
    /// Carries no content of its own — it annotates a sibling flavor. A
    /// `Preferred DropEffect` word, a `public.url-name` title. Publishing it
    /// alone is meaningless.
    Sidecar,
}

/// One flavor to publish, and what publishing it costs.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct WriteFlavor {
    /// The flavor. Resolve it to a platform identifier with
    /// [`native_name`](crate::native_name).
    pub flavor: Flavor<'static>,
    /// What survives the conversion.
    pub fidelity: Fidelity,
    /// For [`Fidelity::Lossy`] and [`Fidelity::Sidecar`], what is lost or what
    /// the flavor is for. Empty for [`Fidelity::Full`].
    pub note: &'static str,
}

const fn full(flavor: Flavor<'static>) -> WriteFlavor {
    WriteFlavor {
        flavor,
        fidelity: Fidelity::Full,
        note: "",
    }
}

const fn lossy(flavor: Flavor<'static>, note: &'static str) -> WriteFlavor {
    WriteFlavor {
        flavor,
        fidelity: Fidelity::Lossy,
        note,
    }
}

const fn sidecar(flavor: Flavor<'static>, note: &'static str) -> WriteFlavor {
    WriteFlavor {
        flavor,
        fidelity: Fidelity::Sidecar,
        note,
    }
}

/// Lifted out because it is the same sentence in six places, and because
/// "the styling is gone" is the single most common loss in the table.
const FLATTENED: &str = "styling is flattened away; the characters survive";

const TEXT: &[WriteFlavor] = &[full(Flavor::PlainText)];

// Windows: RTF first, matching `read_rank`. The plan calls RTF the
// higher-fidelity of the two on Windows (`plan/PLAN.md` §4.3) and there is no
// reason for the read and write tables to contradict each other on a question
// they both have an opinion about.
const RICH_TEXT_WINDOWS: &[WriteFlavor] = &[
    full(Flavor::Rtf),
    full(Flavor::Html),
    lossy(Flavor::PlainText, FLATTENED),
];

// macOS: `public.rtf` is *the* rich flavor. Pages, TextEdit, Mail and Notes all
// speak it and several speak no HTML at all.
//
// TODO(phase-5): whether to add `public.html` here. AppKit's own rich-text
// writers do not, and the failure mode of guessing wrong — TextEdit pasting raw
// markup — is bad enough to want the AppKit oracle test from `plan/PLAN.md`
// §5d before committing to it.
const RICH_TEXT_MACOS: &[WriteFlavor] = &[full(Flavor::Rtf), lossy(Flavor::PlainText, FLATTENED)];

// X11 / Wayland: GTK and Qt both negotiate rich text as `text/html`.
//
// TODO(phase-5): `text/rtf` as a third offer. LibreOffice and AbiWord read it,
// but no capture in the corpus shows a Linux application *offering* it, and
// this table should follow observed behaviour rather than lead it.
const RICH_TEXT_UNIX: &[WriteFlavor] = &[full(Flavor::Html), lossy(Flavor::PlainText, FLATTENED)];

// A fragment the caller handed us as markup. There is no HTML tokenizer in this
// workspace, so the plain-text companion can only be published when the caller
// supplied one; the encoder skips it otherwise rather than shipping tag soup.
const HTML: &[WriteFlavor] = &[
    full(Flavor::Html),
    lossy(
        Flavor::PlainText,
        "published only when the caller supplied a plain-text fallback",
    ),
];

const IMAGE_WINDOWS: &[WriteFlavor] = &[
    full(Flavor::DibV5),
    full(Flavor::Png),
    lossy(
        Flavor::Dib,
        "BITMAPINFOHEADER has no alpha channel; the image goes opaque",
    ),
];

const IMAGE_MACOS: &[WriteFlavor] = &[full(Flavor::Png), full(Flavor::Tiff)];

// `image/bmp` is deliberately absent. A BMP on the X11 or Wayland clipboard is
// a *file* — `BM` magic, 14-byte BITMAPFILEHEADER — and `CF_DIB` is precisely
// the same bytes with that header removed. `rclip-dib` writes only the packed
// form, so offering `image/bmp` would advertise something gdk-pixbuf and Qt
// cannot open. `// TODO(phase-5):` re-wrap as a file header here if a real
// consumer ever asks for it.
const IMAGE_UNIX: &[WriteFlavor] = &[full(Flavor::Png)];

const FILES_WINDOWS: &[WriteFlavor] = &[
    full(Flavor::FileList),
    sidecar(
        Flavor::DropEffect,
        "cut vs copy; without it every paste reads as a copy",
    ),
];

// One `public.file-url` per file. See `crate::encode` for the caveat: a macOS
// pasteboard models this as N *items*, and `ClipboardPayload` is a flat list.
const FILES_MACOS: &[WriteFlavor] = &[full(Flavor::FileList)];

// `rclip_uri_list::emit::RECOMMENDED`, spelled in this crate's vocabulary.
// All three families, because none of them reads the others': GNOME ignores
// KDE's flag, KDE ignores GNOME's verb line, and a receiver that knows neither
// still gets the files out of `text/uri-list`.
const FILES_UNIX: &[WriteFlavor] = &[
    full(Flavor::FileList),
    full(Flavor::Other("x-special/gnome-copied-files")),
    full(Flavor::Other("x-special/mate-copied-files")),
    sidecar(
        Flavor::Other("application/x-kde-cutselection"),
        "cut vs copy, for KDE; pairs with text/uri-list",
    ),
];

const PROMISED_FILES_WINDOWS: &[WriteFlavor] = &[full(Flavor::FileDescriptor)];

const LINK_WINDOWS: &[WriteFlavor] = &[
    full(Flavor::Url),
    lossy(Flavor::PlainText, "the title is dropped"),
];

const LINK_MACOS: &[WriteFlavor] = &[
    full(Flavor::Url),
    sidecar(Flavor::UrlName, "the link's display title"),
    lossy(Flavor::PlainText, "the title is dropped"),
];

// On X11 and Wayland a URL and a file list are the same MIME type, so a link
// published here comes back from `decode` as a one-entry file list. That is not
// a bug in this crate; it is what `text/uri-list` is.
const LINK_UNIX: &[WriteFlavor] = &[
    full(Flavor::Url),
    lossy(Flavor::PlainText, "the title is dropped"),
];

const NONE: &[WriteFlavor] = &[];

/// The flavors to publish for `kind` on `platform`, best first.
///
/// An empty slice means the item has no clipboard representation there. That is
/// a statement about the platform, not a failure: promised files on X11 travel
/// as the XDND `XdndDirectSave0` protocol, which is a message exchange and not
/// a byte format, so there is nothing for a codec crate to produce.
///
/// # Not publishable anywhere
///
/// - [`ItemKind::Shortcut`]. [`rclip_core::Flavor::ShellLink`] has no entry in
///   any of the three platform tables, because a `.lnk` is not a clipboard
///   flavor — it reaches an application as a *file*. Reading one is supported
///   (`Shortcut::from_lnk`, behind the `shell-link` feature); the way to
///   publish one is as a promised file whose contents are the `.lnk` bytes,
///   which needs `CFSTR_FILECONTENTS`, which is transport.
/// - [`ItemKind::ShellItems`]. A PIDL names something in *this* machine's shell
///   namespace. Re-publishing one you did not mint is how you hand a receiver a
///   reference to something you never had.
/// - [`ItemKind::Unknown`]. Not in the table because its flavor is not known at
///   compile time; [`encode`](crate::encode) republishes it verbatim under the
///   identifier it arrived with.
#[must_use]
pub const fn write_plan(kind: ItemKind, platform: Platform) -> &'static [WriteFlavor] {
    match (kind, platform) {
        (ItemKind::Text, _) => TEXT,

        (ItemKind::RichText, Platform::Windows) => RICH_TEXT_WINDOWS,
        (ItemKind::RichText, Platform::MacOs) => RICH_TEXT_MACOS,
        (ItemKind::RichText, Platform::Unix) => RICH_TEXT_UNIX,

        (ItemKind::Html, _) => HTML,

        (ItemKind::Image, Platform::Windows) => IMAGE_WINDOWS,
        (ItemKind::Image, Platform::MacOs) => IMAGE_MACOS,
        (ItemKind::Image, Platform::Unix) => IMAGE_UNIX,

        (ItemKind::Files, Platform::Windows) => FILES_WINDOWS,
        (ItemKind::Files, Platform::MacOs) => FILES_MACOS,
        (ItemKind::Files, Platform::Unix) => FILES_UNIX,

        (ItemKind::PromisedFiles, Platform::Windows) => PROMISED_FILES_WINDOWS,
        // `NSFilePromiseProvider` and XDND's `XdndDirectSave0` are protocols,
        // not byte layouts. Nothing here to serialize.
        (ItemKind::PromisedFiles, _) => NONE,

        (ItemKind::Link, Platform::Windows) => LINK_WINDOWS,
        (ItemKind::Link, Platform::MacOs) => LINK_MACOS,
        (ItemKind::Link, Platform::Unix) => LINK_UNIX,

        (ItemKind::Shortcut | ItemKind::ShellItems | ItemKind::Unknown, _) => NONE,
    }
}

/// Every flavor `kind` can be published as on `platform`, without the fidelity
/// annotations.
///
/// The shape a transport actually wants when all it has to do is advertise a
/// `TARGETS` list.
pub fn write_flavors(
    kind: ItemKind,
    platform: Platform,
) -> impl Iterator<Item = Flavor<'static>> + 'static {
    write_plan(kind, platform).iter().map(|w| w.flavor)
}

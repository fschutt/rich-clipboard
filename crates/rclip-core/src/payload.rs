//! What a clipboard actually hands you: several encodings of the same thing.
//!
//! A clipboard read is never one blob. Copy a table out of a browser and the
//! source offers HTML, RTF, an image and plain text simultaneously, and picking
//! among them is the consumer's decision, not the transport's. [`ClipboardPayload`]
//! is that set, still encoded — decoding is each codec crate's job, and doing it
//! eagerly for flavors nobody asked for would be wasted work on payloads that
//! are routinely megabytes.
//!
//! The flavor is stored as the platform-native identifier the OS reported and
//! resolved on demand, rather than as an owned mirror of [`Flavor`]. That keeps
//! one source of truth for the registry: a new flavor is added in one place, and
//! an identifier this build does not recognise still round-trips verbatim
//! instead of being flattened to "unknown".

extern crate alloc;

use alloc::{string::String, vec::Vec};

use crate::flavor::{Flavor, Platform};

/// One encoding of the clipboard's contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardItem {
    /// The format identifier exactly as the OS reported it.
    ///
    /// Kept verbatim rather than normalised so that an identifier this build
    /// does not know — a private format, a macOS `dyn.` UTI, a vendor MIME
    /// type — can still be handed back to the OS unchanged when writing.
    pub native: String,
    /// The payload, undecoded.
    pub bytes: Vec<u8>,
    /// Which pasteboard item this is a representation *of*.
    ///
    /// macOS pasteboards hold **items**, and each item offers several
    /// representations of one thing: copying three files in Finder produces
    /// three items that each offer `public.file-url`, not one item offering
    /// it three times. Windows and X11/Wayland have no equivalent — one
    /// selection, many flavors — so this stays `0` there.
    ///
    /// It matters because the obvious macOS API gets it wrong:
    /// `-[NSPasteboard dataForType:]` reaches only the *first* item offering a
    /// type, so a three-file copy reads back as one file. Recording the index
    /// is what lets a consumer reassemble the selection instead of silently
    /// losing most of it.
    pub item: usize,
}

impl ClipboardItem {
    /// A representation of the first (usually only) item.
    pub fn new(native: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self::in_item(0, native, bytes)
    }

    /// A representation of item `item`.
    pub fn in_item(item: usize, native: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            native: native.into(),
            bytes: bytes.into(),
            item,
        }
    }

    /// Resolve the native identifier against the registry.
    ///
    /// Borrows `self`, so an unrecognised identifier comes back as
    /// [`Flavor::Other`] pointing at [`ClipboardItem::native`] rather than
    /// being lost.
    #[must_use]
    pub fn flavor(&self, platform: Platform) -> Flavor<'_> {
        Flavor::from_native(platform, &self.native)
    }
}

/// Every encoding a clipboard offered, in the order the source listed them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardPayload {
    platform: Platform,
    items: Vec<ClipboardItem>,
}

impl ClipboardPayload {
    #[must_use]
    pub const fn new(platform: Platform) -> Self {
        Self {
            platform,
            items: Vec::new(),
        }
    }

    #[must_use]
    pub const fn platform(&self) -> Platform {
        self.platform
    }

    pub fn push(&mut self, item: ClipboardItem) -> &mut Self {
        self.items.push(item);
        self
    }

    pub fn with(mut self, native: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        self.items.push(ClipboardItem::new(native, bytes));
        self
    }

    /// Add a representation belonging to item `item`.
    #[must_use]
    pub fn with_in(
        mut self,
        item: usize,
        native: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Self {
        self.items.push(ClipboardItem::in_item(item, native, bytes));
        self
    }

    #[must_use]
    pub fn items(&self) -> &[ClipboardItem] {
        &self.items
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Every flavor on offer, resolved.
    pub fn flavors(&self) -> impl Iterator<Item = Flavor<'_>> {
        self.items.iter().map(|i| i.flavor(self.platform))
    }

    /// The bytes for a specific flavor, if the source offered it.
    ///
    /// First match wins: a source that advertises the same flavor twice is
    /// malformed, and the first listing is the one it meant.
    #[must_use]
    pub fn get(&self, want: Flavor<'_>) -> Option<&ClipboardItem> {
        self.items.iter().find(|i| i.flavor(self.platform) == want)
    }

    /// How many pasteboard items this payload covers.
    ///
    /// One for every platform but macOS, and for most macOS pastes too — it
    /// is a multi-file or multi-object selection that makes this interesting.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.items.iter().map(|i| i.item + 1).max().unwrap_or(0)
    }

    /// Every representation belonging to one item.
    pub fn group(&self, item: usize) -> impl Iterator<Item = &ClipboardItem> {
        self.items.iter().filter(move |i| i.item == item)
    }

    /// Every item offering `want`, in order.
    ///
    /// This is what a file list needs: a three-file Finder copy offers
    /// `public.file-url` three times, once per item, and taking only the first
    /// is how a multi-file paste turns into a single-file paste.
    pub fn all<'a>(&'a self, want: Flavor<'a>) -> impl Iterator<Item = &'a ClipboardItem> + 'a {
        let platform = self.platform;
        self.items
            .iter()
            .filter(move |i| i.flavor(platform) == want)
    }

    /// The richest content flavor on offer, by [`Flavor::read_rank`].
    ///
    /// Metadata flavors are skipped — `Preferred DropEffect` is never what a
    /// paste wanted, however highly it might sort.
    #[must_use]
    pub fn best(&self) -> Option<&ClipboardItem> {
        self.items
            .iter()
            .filter(|i| i.flavor(self.platform).is_content())
            .min_by_key(|i| i.flavor(self.platform).read_rank())
    }
}

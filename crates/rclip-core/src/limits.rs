//! Size policy: what to do when someone pastes something enormous.
//!
//! # Can you know the size before the paste?
//!
//! Partly, and it differs per platform — which is why [`SizeHint`] exists
//! rather than a plain `u64`.
//!
//! | Platform | Available in advance | How |
//! |---|---|---|
//! | Windows | **Yes, exact** | `GlobalSize` on the handle from `GetClipboardData`; `IStream::Stat` for a `TYMED_ISTREAM` |
//! | macOS | **Yes, exact** | `-[NSData length]` on what `dataForType:` returns, before copying it into a `Vec` |
//! | X11 | **Yes, a lower bound** | ICCCM: an `INCR` property's value "represents a lower bound on the number of bytes of data in the selection" |
//! | Wayland | **No** | `wl_data_offer.receive` hands over a pipe fd; you read to EOF and the protocol never states a length |
//!
//! On Windows and macOS the data is already resident and owned by the
//! clipboard server, so asking its size costs nothing and no copy has happened
//! yet. X11 tells you a floor before the incremental transfer starts. Wayland
//! tells you nothing, so the only defence there is to count bytes as they
//! arrive and stop — which is what [`Budget`] is for.
//!
//! # What this does and does not protect
//!
//! Codecs already reject impossible *fields* — a length past the end of the
//! buffer, a count that could not be backed by the remaining input, a pixel
//! count over [`crate::MAX_PIXELS`]. What they do not bound is the
//! *aggregate*: an `ITEMIDLIST` of 64 million individually-valid minimum-size
//! items is well formed at every step, and walking it is fast and allocation-
//! free — but a caller who collects it is holding 64 million items.
//!
//! So this module bounds the whole, and hands the decision to the application
//! rather than making it. A paint program pasting a 400 MB image is normal; a
//! text field receiving one is not, and only the application knows which it is.

use crate::flavor::Flavor;

/// What is known about a payload's size before reading it.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SizeHint {
    /// The exact byte count, from the platform. Windows and macOS give this.
    Exact(u64),
    /// A floor, and the real size may be larger. X11's `INCR` gives this.
    AtLeast(u64),
    /// Nothing is known until the bytes have been read. Wayland gives this.
    Unknown,
}

impl SizeHint {
    /// The largest value this hint rules out, for a conservative check.
    ///
    /// `Unknown` returns `None`: a caller must not treat "no information" as
    /// "small", which is the mistake that makes a Wayland paste the way in.
    #[must_use]
    pub const fn known_bytes(self) -> Option<u64> {
        match self {
            Self::Exact(n) | Self::AtLeast(n) => Some(n),
            Self::Unknown => None,
        }
    }

    /// `true` if this hint alone proves the payload exceeds `limit`.
    ///
    /// Note the asymmetry: `AtLeast` can prove a payload is *too big* but
    /// never that it is small enough, and `Unknown` can prove neither.
    #[must_use]
    pub const fn definitely_exceeds(self, limit: u64) -> bool {
        match self {
            Self::Exact(n) | Self::AtLeast(n) => n > limit,
            Self::Unknown => false,
        }
    }
}

/// What the application wants done about an oversize payload.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Oversize {
    /// Take it anyway. The application has decided it can afford this one.
    Accept,
    /// Drop this flavor and carry on with the rest of the paste. Usually the
    /// right answer: dropping a 400 MB TIFF still leaves the plain text.
    Skip,
    /// Abandon the whole paste.
    Abort,
}

/// Caps for one clipboard operation.
///
/// The defaults are chosen to be generous enough that no realistic paste trips
/// them, and small enough that a hostile one does. They are a backstop, not a
/// policy — an application with an opinion should set its own.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Largest single flavor payload, in bytes. Default 64 MiB.
    pub max_flavor_bytes: u64,
    /// Largest total across every flavor of one paste. Default 256 MiB.
    ///
    /// Separate from the per-flavor cap because a source may offer the same
    /// content a dozen ways — a real Safari copy offers eleven — and the sum
    /// is what the application actually holds.
    pub max_total_bytes: u64,
    /// Largest number of elements a list-shaped payload may yield: files in a
    /// drop, items in an `ITEMIDLIST`, runs in a rich-text document.
    /// Default 1 << 20.
    ///
    /// This is the cap that the per-field checks cannot express, because every
    /// individual element is valid.
    pub max_items: usize,
    /// Largest decoded image, in pixels. Default [`crate::MAX_PIXELS`].
    pub max_pixels: u64,
    /// Largest nesting depth. Default [`crate::MAX_DEPTH`].
    pub max_depth: u32,
}

impl Limits {
    /// No caps at all.
    ///
    /// For a tool that has to accept whatever it is given — a clipboard
    /// inspector, a forensic reader. Not for an interactive application.
    pub const UNLIMITED: Self = Self {
        max_flavor_bytes: u64::MAX,
        max_total_bytes: u64::MAX,
        max_items: usize::MAX,
        max_pixels: u64::MAX,
        max_depth: u32::MAX,
    };

    /// Whether `hint` alone is enough to reject this flavor.
    #[must_use]
    pub const fn rejects(&self, hint: SizeHint) -> bool {
        hint.definitely_exceeds(self.max_flavor_bytes)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_flavor_bytes: 64 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
            max_items: 1 << 20,
            max_pixels: crate::MAX_PIXELS,
            max_depth: crate::MAX_DEPTH,
        }
    }
}

/// The application's answer to "this paste is enormous, what now?".
///
/// Implemented by the consumer, called by the transport before it commits to
/// reading a payload. The default implementation skips anything oversize,
/// which is the safe answer for an application that has not thought about it.
pub trait OversizePolicy {
    /// A payload exceeds the limits. `hint` is what the platform could say
    /// about its size — see [`SizeHint`] for what each platform offers.
    fn on_oversize(&mut self, flavor: Flavor<'_>, hint: SizeHint, limits: &Limits) -> Oversize {
        let _ = (flavor, hint, limits);
        Oversize::Skip
    }
}

/// A closure is a policy, so the common case needs no type.
impl<F: FnMut(Flavor<'_>, SizeHint, &Limits) -> Oversize> OversizePolicy for F {
    fn on_oversize(&mut self, flavor: Flavor<'_>, hint: SizeHint, limits: &Limits) -> Oversize {
        self(flavor, hint, limits)
    }
}

/// A running byte count, for the case where nothing is known in advance.
///
/// On Wayland a payload arrives down a pipe with no declared length, so the
/// only way to bound it is to count while reading and stop. Feed each chunk to
/// [`Budget::consume`] and stop when it returns `false`.
///
/// This is also the right shape for X11's `INCR`, where the advertised lower
/// bound may understate the real size — the floor gets you an early rejection,
/// and the budget catches an owner that sends more than it promised.
#[derive(Debug, Clone)]
pub struct Budget {
    remaining: u64,
    spent: u64,
}

impl Budget {
    #[must_use]
    pub const fn new(limit: u64) -> Self {
        Self {
            remaining: limit,
            spent: 0,
        }
    }

    /// Account for `n` more bytes. `false` means the budget is exhausted and
    /// the caller must stop reading.
    pub fn consume(&mut self, n: u64) -> bool {
        self.spent = self.spent.saturating_add(n);
        match self.remaining.checked_sub(n) {
            Some(rest) => {
                self.remaining = rest;
                true
            }
            None => {
                self.remaining = 0;
                false
            }
        }
    }

    #[must_use]
    pub const fn spent(&self) -> u64 {
        self.spent
    }

    #[must_use]
    pub const fn remaining(&self) -> u64 {
        self.remaining
    }

    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        self.remaining == 0
    }
}

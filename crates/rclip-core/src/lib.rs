//! Shared vocabulary for the `rich-clipboard` codecs.
//!
//! Every codec crate in this workspace depends on this one and on nothing else.
//! It provides three things:
//!
//! - [`Flavor`] and the platform registry — what a clipboard item *is*,
//!   independent of what Win32, `NSPasteboard` or a MIME string call it.
//! - [`Reader`] — a bounds-checked cursor. Codecs read through it rather than
//!   indexing slices, because clipboard payloads come from other processes and
//!   a raw index on a wire-read length field is a panic waiting to happen.
//! - [`Error`] — one error type, always carrying the offset it failed at.
//!
//! # Conventions for codec crates
//!
//! - `#![no_std]` and `#![forbid(unsafe_code)]`, without exception.
//! - Parsing borrows from the input and does not allocate. Anything that must
//!   own its output — serializers, lossy string decoding — goes behind the
//!   `alloc` feature.
//! - Never size an allocation or a loop from a length field without checking it
//!   against the remaining input first ([`Reader::check_count`]).
//! - Bound recursion explicitly and return [`ErrorKind::DepthLimit`]. Never
//!   overflow the stack.
//! - Parsers return data. They never resolve a path, touch a file, or launch
//!   anything.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

#[cfg(feature = "std")]
extern crate std;

pub mod error;
pub mod flavor;
#[cfg(feature = "alloc")]
pub mod payload;
pub mod reader;
pub mod shortcut;
pub mod utf16;

pub use error::{Error, ErrorKind, Result};
pub use flavor::{Flavor, Platform, WindowsFormat};
#[cfg(feature = "alloc")]
pub use payload::{ClipboardItem, ClipboardPayload};
pub use reader::Reader;
pub use shortcut::ShortcutTarget;
pub use utf16::Utf16Le;

/// Depth limit for every recursive parser in the workspace.
///
/// One shared constant so a hostile input cannot find the one codec that forgot
/// to pick a number. Real content nests nowhere near this deep.
pub const MAX_DEPTH: u32 = 64;

/// Largest pixel count any image codec will decode, ~256 megapixels.
///
/// A twelve-byte DIB header can claim a four-gigapixel image; this is the
/// ceiling that keeps that from becoming an allocation.
pub const MAX_PIXELS: u64 = 1 << 28;

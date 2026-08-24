//! Typed clipboard and drag-and-drop payloads.
//!
//! # The three layers
//!
//! A clipboard has three layers, and confusing them is why most clipboard code
//! is hard to test. **Transport** is `IDataObject`, `NSPasteboard`, ICCCM
//! selections and `wl_data_offer` — OS calls, a display server, and no way to
//! unit-test any of it. **Codecs** are `&[u8] -> T` and back: the twelve
//! `rclip-*` crates in this workspace, each one a byte format with no idea what
//! an operating system is. **Policy** is the layer in between, and it is what
//! this crate is: given the several encodings a source offered, which one do you
//! decode; and given one thing the user wants to publish, which set of flavors
//! do you hand the transport so the paste lands as styled text in Word rather
//! than as a flat string. This crate never calls the OS, and the codecs it
//! dispatches to never allocate from a length field. What is left for policy is
//! two tables and the conversions between them.
//!
//! # Reading
//!
//! A source offers everything it can: copy a table out of a browser and you are
//! handed HTML, RTF, an image and plain text at once. [`decode_payload`] picks
//! among them — richest first, skipping anything this build was not compiled to
//! understand — and hands back one [`RichItem`].
//!
//! ```
//! # #[cfg(all(feature = "std", feature = "rtf"))] {
//! use rclip_core::{ClipboardPayload, Platform};
//! use rich_clipboard::{decode_payload, RichItem};
//!
//! let payload = ClipboardPayload::new(Platform::MacOs)
//!     .with("public.utf8-plain-text", &b"hello"[..])
//!     .with("public.rtf", &br"{\rtf1\ansi\b hello\b0}"[..]);
//!
//! // RTF outranks plain text, so the styling survives.
//! match decode_payload(&payload).unwrap() {
//!     RichItem::RichText(text) => assert!(text.runs[0].style.bold),
//!     other => panic!("expected styled text, got {other:?}"),
//! }
//! # }
//! ```
//!
//! # Writing
//!
//! The write side is where the value is, and it is not the read side reversed.
//! One [`RichItem`] becomes *several* flavors at once — three on Windows for
//! styled text — because the receiving application picks, not you.
//!
//! ```
//! # #[cfg(all(feature = "std", feature = "rich-text"))] {
//! use rclip_core::{Flavor, Platform};
//! use rich_clipboard::{encode, RichItem, RichText, Style};
//!
//! let mut text = RichText::default();
//! text.push("bold", Style { bold: true, ..Style::default() });
//!
//! let payload = encode(&RichItem::RichText(text), Platform::Windows).unwrap();
//! let flavors: Vec<_> = payload.flavors().collect();
//! assert_eq!(flavors, [Flavor::Rtf, Flavor::Html, Flavor::PlainText]);
//! # }
//! ```
//!
//! [`fanout::write_plan`] is that table, and it is public: a transport that
//! wants to know what it is about to publish, and what each flavor costs, can
//! ask before it commits. It is also the half of the crate that needs no
//! allocator, so a `no_std` build still gets the policy even without the
//! codecs.
//!
//! # Lossiness
//!
//! Most conversions here lose something, and every one that does says so twice:
//! [`fanout::WriteFlavor::fidelity`] carries it in the type system for the write
//! side, and every conversion function has a `# What is lost` section. The one
//! rule with no exceptions is that a conversion never invents information — a
//! `.desktop` file's target is a string that *looks* like a path, never a path
//! that exists, and nothing in this workspace touches a filesystem.
//!
//! # Feature layout
//!
//! Every format is behind a feature and every format is off by default. An
//! application that pastes text and images must not compile a `.lnk` parser to
//! do it. See the crate README for the table; `full` turns on every format.
//!
//! `image` is outside `full` and is not a format: it is the delegation
//! `plan/PLAN.md` §4.4 asks for, letting `Image::Rgba` be encoded as PNG and
//! TIFF so it has a macOS representation at all. It is the one dependency here
//! that is not a `rclip-*` codec, which is why turning it on is a decision
//! rather than part of "everything".
//!
//! A flavor whose feature is off is not silently skipped: [`decode`] returns
//! [`Error::FeatureDisabled`] naming the Cargo feature to turn on, and
//! [`decode_payload`] moves on to the next-best flavor and only reports it if
//! nothing else worked.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

pub mod fanout;

#[cfg(feature = "alloc")]
mod decode;
#[cfg(feature = "alloc")]
mod encode;
#[cfg(feature = "alloc")]
mod error;
#[cfg(feature = "alloc")]
mod item;
#[cfg(all(feature = "alloc", feature = "shell-link"))]
mod lnk;
#[cfg(feature = "alloc")]
mod native;
#[cfg(feature = "alloc")]
pub mod rich_text;
#[cfg(feature = "alloc")]
pub mod shortcut;
#[cfg(feature = "alloc")]
mod text;

pub use fanout::{write_flavors, write_plan, Fidelity, ItemKind, WriteFlavor};

#[cfg(feature = "alloc")]
pub use decode::{
    decode, decode_all, decode_payload, decode_payload_with, decode_with, transfer_action, Options,
};
#[cfg(feature = "alloc")]
pub use encode::{encode, encode_with};
#[cfg(feature = "alloc")]
pub use error::{Error, Result};
#[cfg(feature = "alloc")]
pub use item::{
    FileEntry, FileList, HtmlFragment, Image, ImageFormat, PromisedFile, RgbaImage, RichItem,
    ShellItems, Shortcut, TransferAction,
};
#[cfg(feature = "alloc")]
pub use native::native_name;
#[cfg(feature = "alloc")]
pub use rich_text::{Rgb, RichText, Style, StyledRun};
#[cfg(feature = "alloc")]
pub use shortcut::{Link, LinkTarget};

// `rclip-core`'s vocabulary, re-exported so a consumer needs one dependency
// rather than two. The payload types are behind `alloc` there and so here.
#[cfg(feature = "alloc")]
pub use rclip_core::{ClipboardItem, ClipboardPayload};
pub use rclip_core::{Flavor, Platform};

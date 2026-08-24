//! What a backend hands back, and how a failure is reported.
//!
//! Deliberately dumb: a backend's whole job is to turn whatever the platform
//! calls "the clipboard" into a list of `(native identifier, bytes)` and say
//! nothing about what any of it means. Resolving identifiers to [`Flavor`]s,
//! naming files and writing sidecars all happen once, above this line, so the
//! four backends cannot drift from each other in the parts that are not
//! platform-specific.
//!
//! [`Flavor`]: rclip_core::Flavor

use std::fmt;

/// Which selection to read.
///
/// Windows and macOS have exactly one clipboard; X11 and Wayland also have the
/// select-to-copy / middle-click-to-paste PRIMARY selection, which is a
/// different set of bytes from a different application and is worth capturing
/// separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Clipboard,
    Primary,
}

impl Selection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::Primary => "primary",
        }
    }
}

/// One format the clipboard is offering.
#[derive(Debug, Clone)]
pub struct Offered {
    /// The identifier exactly as the OS reported it: a UTI, a Win32 format
    /// name, an X11 target name, a MIME type. Never normalised — the whole
    /// point of the capture is what the source actually said.
    pub native: String,
    /// Index into a multi-item pasteboard, where the platform has such a thing.
    ///
    /// `None` on every platform but macOS, and on macOS only set when the
    /// pasteboard carries more than one `NSPasteboardItem` — copying three
    /// files in Finder yields three items, each with its own `public.file-url`,
    /// and the pasteboard-level API can only ever see the first.
    pub item: Option<usize>,
    pub body: Body,
    /// Anything the backend learned about *this* item that the bytes do not
    /// say: the X11 property type and whether it arrived through INCR, the
    /// fact that a Windows length came from `GlobalSize` and may be rounded
    /// up. Appended to the sidecar's `notes`, because a capture that cannot
    /// explain itself six months later is not much of a capture.
    pub detail: Option<String>,
}

/// The bytes, or the reason there are none.
#[derive(Debug, Clone)]
pub enum Body {
    Bytes(Vec<u8>),
    /// Offered, but nothing was dumped. `CF_BITMAP` is a GDI handle rather
    /// than a byte range; `DELETE` is an X11 target that would destroy the
    /// selection if requested; a promised file may simply not be delivered.
    /// All three are worth *reporting* — a format silently missing from the
    /// output is the one thing this tool must never do.
    Skipped(String),
}

/// Everything one backend found.
#[derive(Debug)]
pub struct Capture {
    /// Which naming vocabulary [`Offered::native`] is written in.
    pub platform: rclip_core::Platform,
    /// Human-readable description of where these bytes came from, for the
    /// sidecar: `"NSPasteboard general"`, `"X11 CLIPBOARD via INCR"`, …
    pub source: String,
    pub offered: Vec<Offered>,
}

/// Boxed because backends fail in four unrelated vocabularies (`io::Error`,
/// `ReplyError`, `ConnectError`, a `BOOL` and `GetLastError`) and the only
/// thing `main` does with any of them is print it.
pub type Error = Box<dyn std::error::Error>;
pub type Result<T> = std::result::Result<T, Error>;

/// An error with no underlying source: a refusal, or a platform that has no
/// such concept.
#[derive(Debug)]
pub struct Plain(pub String);

impl fmt::Display for Plain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Plain {}

/// `bail!("...")` — an early return with a formatted message.
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err(Box::new($crate::capture::Plain(format!($($arg)*))) as $crate::capture::Error)
    };
}

pub(crate) use bail;

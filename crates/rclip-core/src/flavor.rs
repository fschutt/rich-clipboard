//! The cross-platform flavor registry.
//!
//! A [`Flavor`] is the abstract thing on a clipboard — "rich text", "a list of
//! files" — independent of what any one OS calls it. This module holds the
//! mapping to and from each platform's identifier, because that mapping *is*
//! the cross-platform knowledge and belongs in one place rather than repeated
//! in four backends.
//!
//! See `plan/PLAN.md` §4.1 for the table this encodes.

/// A predefined Win32 clipboard format number, or a name that must be passed to
/// `RegisterClipboardFormat` at runtime.
///
/// Kept as data rather than a `u32` because the registered ones have no stable
/// numeric value — they differ per session.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum WindowsFormat {
    /// A `CF_*` constant with a fixed value.
    Predefined(u32),
    /// A string passed to `RegisterClipboardFormat`.
    Registered(&'static str),
}

/// `CF_*` constants, from `winuser.h`.
pub mod cf {
    pub const TEXT: u32 = 1;
    pub const BITMAP: u32 = 2;
    pub const METAFILEPICT: u32 = 3;
    pub const SYLK: u32 = 4;
    pub const DIF: u32 = 5;
    pub const TIFF: u32 = 6;
    pub const OEMTEXT: u32 = 7;
    pub const DIB: u32 = 8;
    pub const PALETTE: u32 = 9;
    pub const PENDATA: u32 = 10;
    pub const RIFF: u32 = 11;
    pub const WAVE: u32 = 12;
    pub const UNICODETEXT: u32 = 13;
    pub const ENHMETAFILE: u32 = 14;
    pub const HDROP: u32 = 15;
    pub const LOCALE: u32 = 16;
    pub const DIBV5: u32 = 17;
}

/// `CFSTR_*` registered format names, from `shlobj.h`.
pub mod cfstr {
    pub const HTML: &str = "HTML Format";
    pub const RTF: &str = "Rich Text Format";
    pub const RTF_NO_OBJS: &str = "Rich Text Format Without Objects";
    pub const PNG: &str = "PNG";
    pub const JFIF: &str = "JFIF";
    pub const GIF: &str = "GIF";
    pub const SHELLIDLIST: &str = "Shell IDList Array";
    pub const FILEDESCRIPTORW: &str = "FileGroupDescriptorW";
    pub const FILEDESCRIPTORA: &str = "FileGroupDescriptor";
    pub const FILECONTENTS: &str = "FileContents";
    pub const FILENAMEW: &str = "FileNameW";
    pub const FILENAMEA: &str = "FileName";
    pub const INETURL: &str = "UniformResourceLocatorW";
    pub const INETURL_A: &str = "UniformResourceLocator";
    pub const PREFERREDDROPEFFECT: &str = "Preferred DropEffect";
    pub const PERFORMEDDROPEFFECT: &str = "Performed DropEffect";
    pub const LOGICALPERFORMEDDROPEFFECT: &str = "Logical Performed DropEffect";
    pub const PASTESUCCEEDED: &str = "Paste Succeeded";
    pub const MOUNTEDVOLUME: &str = "MountedVolume";
    pub const TARGETCLSID: &str = "TargetCLSID";
    pub const UNTRUSTEDDRAGDROP: &str = "UntrustedDragDrop";
}

/// `DROPEFFECT_*` bits, carried by `CFSTR_PREFERREDDROPEFFECT` and friends.
pub mod drop_effect {
    pub const NONE: u32 = 0;
    pub const COPY: u32 = 1;
    pub const MOVE: u32 = 2;
    pub const LINK: u32 = 4;
}

/// What is on the clipboard, abstractly.
///
/// Borrowed rather than owning so the whole registry works without `alloc`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Flavor<'a> {
    /// UTF-8 plain text.
    PlainText,
    /// An HTML fragment. On Windows this is `CF_HTML` and needs its header
    /// stripped; elsewhere it is bare markup.
    Html,
    /// Rich Text Format.
    Rtf,

    Png,
    Jpeg,
    Gif,
    Tiff,
    /// Windows device-independent bitmap, `BITMAPINFOHEADER` form.
    Dib,
    /// Windows device-independent bitmap, `BITMAPV5HEADER` form (has alpha).
    DibV5,

    /// A list of existing files. `CF_HDROP` / `public.file-url` / `text/uri-list`.
    FileList,
    /// Shell namespace objects as PIDLs. Windows only.
    ShellIdList,
    /// Descriptors for files that do not exist on disk yet.
    FileDescriptor,
    /// The bytes of one virtual file named by a `FileDescriptor`.
    FileContents,

    /// A single URL.
    Url,
    /// The human-readable title that goes with [`Flavor::Url`].
    UrlName,
    /// A serialized shell link (`.lnk`).
    ShellLink,

    /// A `DROPEFFECT` word saying whether the source cut or copied.
    DropEffect,

    /// Anything not in the table above, by its platform-native name.
    Other(&'a str),
}

impl<'a> Flavor<'a> {
    /// The Win32 format this maps to, if any.
    #[must_use]
    pub const fn windows(&self) -> Option<WindowsFormat> {
        use WindowsFormat::{Predefined, Registered};
        Some(match self {
            Self::PlainText => Predefined(cf::UNICODETEXT),
            Self::Html => Registered(cfstr::HTML),
            Self::Rtf => Registered(cfstr::RTF),
            Self::Png => Registered(cfstr::PNG),
            Self::Jpeg => Registered(cfstr::JFIF),
            Self::Gif => Registered(cfstr::GIF),
            Self::Tiff => Predefined(cf::TIFF),
            Self::Dib => Predefined(cf::DIB),
            Self::DibV5 => Predefined(cf::DIBV5),
            Self::FileList => Predefined(cf::HDROP),
            Self::ShellIdList => Registered(cfstr::SHELLIDLIST),
            Self::FileDescriptor => Registered(cfstr::FILEDESCRIPTORW),
            Self::FileContents => Registered(cfstr::FILECONTENTS),
            Self::Url => Registered(cfstr::INETURL),
            Self::DropEffect => Registered(cfstr::PREFERREDDROPEFFECT),
            Self::UrlName | Self::ShellLink | Self::Other(_) => return None,
        })
    }

    /// The macOS Uniform Type Identifier this maps to, if any.
    #[must_use]
    pub const fn uti(&self) -> Option<&'static str> {
        Some(match self {
            Self::PlainText => "public.utf8-plain-text",
            Self::Html => "public.html",
            Self::Rtf => "public.rtf",
            Self::Png => "public.png",
            Self::Jpeg => "public.jpeg",
            Self::Gif => "com.compuserve.gif",
            Self::Tiff => "public.tiff",
            Self::FileList => "public.file-url",
            Self::Url => "public.url",
            Self::UrlName => "public.url-name",
            Self::Dib
            | Self::DibV5
            | Self::ShellIdList
            | Self::FileDescriptor
            | Self::FileContents
            | Self::ShellLink
            | Self::DropEffect
            | Self::Other(_) => return None,
        })
    }

    /// The X11 / Wayland MIME type this maps to, if any.
    ///
    /// X11 selection targets and Wayland MIME strings are the same vocabulary;
    /// only the transport differs.
    #[must_use]
    pub const fn mime(&self) -> Option<&'static str> {
        Some(match self {
            Self::PlainText => "text/plain;charset=utf-8",
            Self::Html => "text/html",
            Self::Rtf => "text/rtf",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Tiff => "image/tiff",
            Self::Dib | Self::DibV5 => "image/bmp",
            Self::FileList => "text/uri-list",
            Self::Url => "text/uri-list",
            Self::ShellIdList
            | Self::FileDescriptor
            | Self::FileContents
            | Self::UrlName
            | Self::ShellLink
            | Self::DropEffect
            | Self::Other(_) => return None,
        })
    }

    /// Recognize an X11 target / Wayland MIME string.
    ///
    /// Falls back to [`Flavor::Other`] rather than failing: an unknown flavor
    /// is still worth carrying through to the application.
    #[must_use]
    pub fn from_mime(s: &'a str) -> Self {
        // Strip any parameters (`;charset=…`) before matching the essence.
        let essence = match s.find(';') {
            Some(i) => s[..i].trim(),
            None => s.trim(),
        };
        match essence {
            "text/plain" | "UTF8_STRING" | "STRING" | "TEXT" | "text/plain;charset=utf-8" => {
                Self::PlainText
            }
            "text/html" => Self::Html,
            "text/rtf" | "application/rtf" | "text/richtext" => Self::Rtf,
            "image/png" => Self::Png,
            "image/jpeg" => Self::Jpeg,
            "image/gif" => Self::Gif,
            "image/tiff" => Self::Tiff,
            "image/bmp" | "image/x-bmp" | "image/x-MS-bmp" => Self::Dib,
            "text/uri-list" => Self::FileList,
            _ => Self::Other(s),
        }
    }

    /// Recognize a macOS UTI, including the legacy `NSPasteboardType` names
    /// that still show up on real pasteboards.
    #[must_use]
    pub fn from_uti(s: &'a str) -> Self {
        match s {
            "public.utf8-plain-text" | "public.plain-text" | "NSStringPboardType" => {
                Self::PlainText
            }
            "public.html" | "Apple HTML pasteboard type" => Self::Html,
            "public.rtf" | "NeXT Rich Text Format v1.0 pasteboard type" => Self::Rtf,
            "public.png" => Self::Png,
            "public.jpeg" => Self::Jpeg,
            "com.compuserve.gif" => Self::Gif,
            "public.tiff" | "NeXT TIFF v4.0 pasteboard type" => Self::Tiff,
            "public.file-url" | "NSFilenamesPboardType" => Self::FileList,
            "public.url" | "Apple URL pasteboard type" => Self::Url,
            "public.url-name" => Self::UrlName,
            _ => Self::Other(s),
        }
    }

    /// Recognize a Win32 format.
    #[must_use]
    pub fn from_windows(f: WindowsFormat) -> Self {
        match f {
            WindowsFormat::Predefined(cf::UNICODETEXT | cf::TEXT | cf::OEMTEXT) => Self::PlainText,
            WindowsFormat::Predefined(cf::TIFF) => Self::Tiff,
            WindowsFormat::Predefined(cf::DIB) => Self::Dib,
            WindowsFormat::Predefined(cf::DIBV5) => Self::DibV5,
            WindowsFormat::Predefined(cf::HDROP) => Self::FileList,
            WindowsFormat::Registered(name) => Self::from_windows_name(name),
            WindowsFormat::Predefined(_) => Self::Other(""),
        }
    }

    fn from_windows_name(name: &'a str) -> Self {
        match name {
            cfstr::HTML => Self::Html,
            cfstr::RTF | cfstr::RTF_NO_OBJS => Self::Rtf,
            cfstr::PNG => Self::Png,
            cfstr::JFIF => Self::Jpeg,
            cfstr::GIF => Self::Gif,
            cfstr::SHELLIDLIST => Self::ShellIdList,
            cfstr::FILEDESCRIPTORW | cfstr::FILEDESCRIPTORA => Self::FileDescriptor,
            cfstr::FILECONTENTS => Self::FileContents,
            cfstr::INETURL | cfstr::INETURL_A => Self::Url,
            cfstr::PREFERREDDROPEFFECT => Self::DropEffect,
            other => Self::Other(other),
        }
    }

    /// Read-preference rank: lower is better.
    ///
    /// When a source offers several flavors, this is the order to try them in.
    /// Richer representations win, because plain text can always be derived
    /// from rich text but never the other way round.
    #[must_use]
    pub const fn read_rank(&self) -> u8 {
        match self {
            Self::ShellLink => 0,
            Self::ShellIdList => 1,
            Self::FileList => 2,
            Self::FileDescriptor => 3,
            Self::Rtf => 4,
            Self::Html => 5,
            Self::Png => 6,
            Self::Tiff => 7,
            Self::DibV5 => 8,
            Self::Dib => 9,
            Self::Jpeg => 10,
            Self::Gif => 11,
            Self::Url => 12,
            Self::PlainText => 13,
            Self::UrlName | Self::FileContents | Self::DropEffect => 14,
            Self::Other(_) => 255,
        }
    }

    /// `true` if this flavor carries the payload rather than metadata about it.
    #[must_use]
    pub const fn is_content(&self) -> bool {
        !matches!(self, Self::DropEffect | Self::UrlName)
    }
}

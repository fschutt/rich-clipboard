//! One error type for every codec in the workspace.
//!
//! Every parse failure carries the byte offset it happened at. That offset is
//! what makes a fuzz crash reproducible and a corpus mismatch debuggable, so no
//! codec should ever return a bare kind.

use core::fmt;

/// What went wrong.
///
/// Deliberately coarse: callers branch on "can I recover" and not on the exact
/// field that was malformed. The offset plus the corpus file is what you debug
/// with.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Ran off the end of the input.
    UnexpectedEof,
    /// A magic number / signature / CLSID did not match.
    BadMagic,
    /// A length or size field is self-contradictory (too small, too large, or
    /// inconsistent with a sibling field).
    BadLength,
    /// An offset field points outside the buffer, or backwards into a region
    /// that was already consumed.
    BadOffset,
    /// A well-formed construct this codec does not implement.
    Unsupported,
    /// Bytes that must be UTF-8 are not.
    InvalidUtf8,
    /// Bytes that must be UTF-16 are not (lone surrogate, odd byte count).
    InvalidUtf16,
    /// Nesting exceeded the codec's depth limit. Always an error, never a
    /// stack overflow.
    DepthLimit,
    /// A length field claims more than the codec is willing to allocate or
    /// iterate. Guards against a 12-byte header declaring a 4-gigapixel image.
    TooLarge,
    /// The input is structurally valid but semantically impossible.
    Malformed,
}

impl ErrorKind {
    /// Human-readable name, for `Display` and for test assertions.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnexpectedEof => "unexpected end of input",
            Self::BadMagic => "bad magic",
            Self::BadLength => "bad length field",
            Self::BadOffset => "bad offset field",
            Self::Unsupported => "unsupported construct",
            Self::InvalidUtf8 => "invalid UTF-8",
            Self::InvalidUtf16 => "invalid UTF-16",
            Self::DepthLimit => "nesting depth limit exceeded",
            Self::TooLarge => "declared size too large",
            Self::Malformed => "malformed",
        }
    }
}

/// A parse failure, located.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Error {
    pub kind: ErrorKind,
    /// Byte offset into the buffer handed to the parser.
    pub offset: usize,
}

impl Error {
    #[must_use]
    pub const fn new(kind: ErrorKind, offset: usize) -> Self {
        Self { kind, offset }
    }

    #[must_use]
    pub const fn eof(offset: usize) -> Self {
        Self::new(ErrorKind::UnexpectedEof, offset)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.kind.as_str(), self.offset)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;

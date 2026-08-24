//! The facade's error type.
//!
//! Deliberately not `rclip_core::Error`. A codec fails for one reason — the
//! bytes were wrong — and reports the offset. The facade fails for four, and
//! three of them are not about the bytes at all: the flavor is one nothing in
//! the workspace decodes, the flavor is one *this build* was not compiled to
//! decode, or the item cannot be expressed on the target platform. A caller
//! that gets "unsupported construct at byte 0" back from a paste cannot tell
//! those apart, and only one of them is fixed by turning on a Cargo feature.

use alloc::string::String;
use core::fmt;

use rclip_core::Platform;

use crate::fanout::ItemKind;

/// What went wrong decoding or encoding a clipboard item.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A codec rejected the bytes. The inner error carries the offset.
    Codec {
        /// The platform-native identifier the item arrived under.
        native: String,
        /// What the codec said.
        source: rclip_core::Error,
    },
    /// The workspace has a codec for this flavor, but this build was not
    /// compiled with it.
    ///
    /// Actionable, which is the whole reason this is not folded into
    /// [`Error::Unsupported`]: turn on `feature`.
    FeatureDisabled {
        /// The flavor, named the way `Debug` would.
        flavor: &'static str,
        /// The Cargo feature to enable on `rich-clipboard`.
        feature: &'static str,
    },
    /// Every flavor on offer was larger than [`Limits`] allows, and the
    /// oversize policy skipped each one.
    ///
    /// [`Limits`]: rclip_core::Limits
    ///
    /// This is a *second* line of defence and says so deliberately: by the time
    /// a `ClipboardPayload` exists the bytes are already resident, because
    /// something read them. Refusing here prevents *decoding* a huge payload —
    /// which is where the amplification is, since a 60 MB 8-bit DIB becomes
    /// 240 MB of RGBA — but it cannot prevent having received it. The transport
    /// applies the first line, before it reads, using
    /// [`SizeHint`](rclip_core::SizeHint).
    Oversize {
        /// The largest flavor that was refused, named the way `Debug` would.
        flavor: &'static str,
        /// Its encoded size.
        bytes: u64,
        /// The per-flavor cap it exceeded.
        limit: u64,
    },
    /// Nothing in this workspace decodes this flavor, whatever features are on.
    Unsupported {
        /// The platform-native identifier.
        native: String,
    },
    /// The flavor carries metadata about a sibling flavor rather than content
    /// of its own — a `Preferred DropEffect` word, a `public.url-name` title.
    ///
    /// [`decode_payload`](crate::decode_payload) folds these into the item they
    /// annotate; asking [`decode`](crate::decode) for one on its own is the
    /// error.
    NotContent {
        /// The platform-native identifier.
        native: String,
    },
    /// The payload offered nothing at all.
    EmptyPayload,
    /// The item has no representation on this platform. See
    /// [`write_plan`](crate::write_plan) for the cases and why.
    NotPublishable {
        /// What was being published.
        kind: ItemKind,
        /// Where it could not be published.
        platform: Platform,
    },
    /// The item could be published in principle, but every flavor in its plan
    /// needed a codec this build does not have, or an encoder that does not
    /// exist yet.
    ///
    /// `missing` names the first feature that would have helped, when one
    /// would have.
    NothingEncodable {
        /// What was being published.
        kind: ItemKind,
        /// Where.
        platform: Platform,
        /// A Cargo feature that would have produced at least one flavor.
        missing: Option<&'static str>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec { native, source } => {
                write!(f, "decoding {native}: {source}")
            }
            Self::FeatureDisabled { flavor, feature } => write!(
                f,
                "{flavor} needs the `{feature}` feature of rich-clipboard, which is off",
            ),
            Self::Oversize {
                flavor,
                bytes,
                limit,
            } => write!(
                f,
                "every flavor exceeded the size limit; largest was {flavor} at {bytes} bytes \
                 against a {limit}-byte cap",
            ),
            Self::Unsupported { native } => {
                write!(f, "no codec for clipboard format {native}")
            }
            Self::NotContent { native } => {
                write!(f, "{native} annotates another flavor and has no content")
            }
            Self::EmptyPayload => f.write_str("the clipboard payload was empty"),
            Self::NotPublishable { kind, platform } => {
                write!(f, "{kind:?} cannot be published on {platform:?}")
            }
            Self::NothingEncodable {
                kind,
                platform,
                missing,
            } => {
                write!(f, "nothing to publish for {kind:?} on {platform:?}")?;
                match missing {
                    Some(feature) => write!(f, "; try the `{feature}` feature"),
                    None => Ok(()),
                }
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl Error {
    /// Wrap a codec failure, recording which item it came from.
    ///
    /// Every caller is behind a format feature, so a build with none of them
    /// on has no way to reach this.
    #[cfg_attr(not(feature = "full"), allow(dead_code))]
    pub(crate) fn codec(native: &str, source: rclip_core::Error) -> Self {
        Self::Codec {
            native: String::from(native),
            source,
        }
    }
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = core::result::Result<T, Error>;

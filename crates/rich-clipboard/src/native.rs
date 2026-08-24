//! Resolving a [`Flavor`] to the identifier a transport has to say out loud.

use rclip_core::{Flavor, Platform};

/// The platform-native identifier for `flavor`, or `None` if the platform has
/// no name for it.
///
/// On Windows the predefined `CF_*` formats come back as their constant's own
/// name — `"CF_UNICODETEXT"`, not `13`. That is deliberate and it is what
/// [`Flavor::from_windows_name`] reads back, so a payload round-trips through
/// [`ClipboardPayload`](rclip_core::ClipboardPayload) without a second code
/// path. The transport turns the name into a number: a predefined format has a
/// fixed one, and a registered format's differs per session and has to come
/// from `RegisterClipboardFormat` anyway.
#[must_use]
pub fn native_name(flavor: Flavor<'_>, platform: Platform) -> Option<&str> {
    if let Flavor::Other(name) = flavor {
        return Some(name);
    }
    match platform {
        Platform::Windows => flavor.windows().and_then(rclip_core::WindowsFormat::name),
        Platform::MacOs => flavor.uti(),
        Platform::Unix => flavor.mime(),
    }
}

//! One backend per platform, selected at compile time.
//!
//! Each is gated on `#[cfg(target_os = ...)]` and its binding crate is a
//! target-specific dependency, so `cargo build` works on any host and links
//! only that host's clipboard API. The unix backend is the exception: X11 and
//! Wayland ship in the same binary because the same Linux build has to run
//! under both, so the choice is made from the environment at run time.
//!
//! A backend returns [`Capture`] and nothing else. It does not decide what a
//! format *is*, what it should be called on disk, or whether it is
//! interesting — see `main.rs` for all three.

use crate::capture::{Capture, Result, Selection};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
mod wayland;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
mod x11;

// -- macOS --------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub const NAME: &str = "macOS";
#[cfg(target_os = "macos")]
pub const HAS_PRIMARY_SELECTION: bool = false;

#[cfg(target_os = "macos")]
pub fn capture(selection: Selection) -> Result<Capture> {
    macos::capture(selection)
}

/// `ProductVersion` and `ProductBuildVersion` out of the system plist.
///
/// Read from the file rather than shelled out to `sw_vers`, which is the same
/// information one process fewer.
#[cfg(target_os = "macos")]
pub fn os_description() -> Option<String> {
    let plist = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
    let version = plist_string(&plist, "ProductVersion")?;
    match plist_string(&plist, "ProductBuildVersion") {
        Some(build) => Some(format!("macOS {version} ({build})")),
        None => Some(format!("macOS {version}")),
    }
}

/// `<key>K</key><string>V</string>` out of an XML plist.
///
/// A hundred lines of plist parser to read two strings from a file Apple has
/// shipped in the same shape since 2001 would be the wrong trade; if the file
/// is ever binary this returns `None` and `--os` covers it.
#[cfg(target_os = "macos")]
fn plist_string(plist: &str, key: &str) -> Option<String> {
    let after_key = plist.split_once(&format!("<key>{key}</key>"))?.1;
    let open = after_key.find("<string>")? + "<string>".len();
    let close = after_key[open..].find("</string>")? + open;
    Some(after_key[open..close].trim().to_owned())
}

// -- Windows ------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub const NAME: &str = "Windows";
#[cfg(target_os = "windows")]
pub const HAS_PRIMARY_SELECTION: bool = false;

#[cfg(target_os = "windows")]
pub fn capture(selection: Selection) -> Result<Capture> {
    windows::capture(selection)
}

/// Windows has no cheap, non-deprecated way to ask for its own version — the
/// `GetVersionEx` family lies unless the binary carries a compatibility
/// manifest — so this reports the family and leaves the detail to `--os`.
#[cfg(target_os = "windows")]
pub fn os_description() -> Option<String> {
    Some("Windows".to_owned())
}

// -- X11 and Wayland ----------------------------------------------------------

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
pub const NAME: &str = "this platform";
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
pub const HAS_PRIMARY_SELECTION: bool = true;

/// Wayland first, X11 second, because a session with `WAYLAND_DISPLAY` set
/// usually also has `DISPLAY` pointing at Xwayland, whose clipboard is a
/// bridged copy of the Wayland one — reading the bridge would capture
/// Xwayland's translation rather than what the application offered.
///
/// `RCLIP_BACKEND=x11` forces the other way round, which is exactly how you
/// capture that translation on purpose.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
pub fn capture(selection: Selection) -> Result<Capture> {
    let forced = std::env::var("RCLIP_BACKEND").ok();
    match forced.as_deref() {
        Some("x11") => return x11::capture(selection),
        Some("wayland") => return wayland::capture(selection),
        Some(other) => {
            crate::capture::bail!("RCLIP_BACKEND={other} is not one of `x11`, `wayland`")
        }
        None => {}
    }

    let wayland_display = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x_display = std::env::var_os("DISPLAY").is_some();
    match (wayland_display, x_display) {
        (true, _) => wayland::capture(selection),
        (false, true) => x11::capture(selection),
        (false, false) => crate::capture::bail!(
            "neither WAYLAND_DISPLAY nor DISPLAY is set: no display server to ask"
        ),
    }
}

/// Selection targets that *do* something rather than return something.
///
/// ICCCM defines several targets as side-effecting requests: converting
/// `DELETE` destroys the selection's contents and `SAVE_TARGETS` asks the
/// clipboard manager to take ownership. A tool whose job is "ask for every
/// target" would otherwise damage the thing it was asked to inspect. The list
/// is shared with the Wayland backend because Xwayland bridges these same
/// names through as MIME types.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
pub fn side_effecting(name: &str) -> Option<&'static str> {
    Some(match name {
        "TARGETS" => "the target list itself; its value is session-local atom ids",
        "MULTIPLE" => "needs a parameter property naming target/property pairs (ICCCM 2.6.2)",
        "SAVE_TARGETS" => "a request that the clipboard manager take ownership, not data",
        "DELETE" => "converting this destroys the selection's contents (ICCCM 2.6.3)",
        "INSERT_SELECTION" | "INSERT_PROPERTY" => "a side-effecting ICCCM target, not data",
        _ => return None,
    })
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
pub fn os_description() -> Option<String> {
    let release = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in release.lines() {
        if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
            return Some(value.trim_matches('"').to_owned());
        }
    }
    None
}

// -- Anything else ------------------------------------------------------------

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(unix, not(any(target_os = "macos", target_os = "ios")))
)))]
mod unsupported {
    use super::{Capture, Result, Selection};

    pub const NAME: &str = "this platform";
    pub const HAS_PRIMARY_SELECTION: bool = false;

    pub fn capture(_selection: Selection) -> Result<Capture> {
        crate::capture::bail!("no clipboard backend is compiled in for this target")
    }

    pub fn os_description() -> Option<String> {
        None
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(unix, not(any(target_os = "macos", target_os = "ios")))
)))]
pub use unsupported::{capture, os_description, HAS_PRIMARY_SELECTION, NAME};

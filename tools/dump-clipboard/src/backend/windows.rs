//! Windows: `OpenClipboard` / `EnumClipboardFormats` / `GetClipboardData`.
//!
//! **Not verified by running** — written from the Win32 documentation and
//! compile-checked for `x86_64-pc-windows-msvc`. Nothing here has touched a
//! real clipboard.
//!
//! Four things this has to get right:
//!
//! * **Predefined formats have no name.** `GetClipboardFormatNameW` only
//!   answers for formats that went through `RegisterClipboardFormat`; for
//!   `CF_UNICODETEXT` and friends it fails, and the number has to be mapped to
//!   its `CF_*` spelling by hand. `rclip_core::flavor::WindowsFormat::name()`
//!   already holds that mapping for every format the registry knows, and its
//!   spellings are exactly what `Flavor::from_windows_name` reads back, so it
//!   is the first thing consulted. [`predefined_name`] only fills in the rest —
//!   `CF_WAVE`, `CF_LOCALE`, the `DSP` family — which resolve to
//!   `Flavor::Other` either way but should still be legible in a capture.
//! * **Not every handle is memory.** `CF_BITMAP` and `CF_PALETTE` are GDI
//!   objects, `CF_ENHMETAFILE` is an `HENHMETAFILE`, `CF_METAFILEPICT` is an
//!   `HGLOBAL` whose contents are mostly *another* handle, and
//!   `CF_OWNERDISPLAY` has no data at all. `GlobalLock` on any of them is
//!   undefined, so they are reported as offered and skipped. Turning an
//!   `HBITMAP` back into bytes means `GetDIBits` and a decision about alpha,
//!   which is exactly the decision `rclip-dib` exists to make and not something
//!   a capture tool should quietly make on its behalf.
//! * **`GlobalLock` and `GlobalUnlock` are a pair,** and the pointer is only
//!   valid between them. The clipboard also has to be closed on every path,
//!   including the error paths, hence the two guards below.
//! * **Some of what you see was not put there by the application.** Windows
//!   synthesises formats on demand — `CF_TEXT` and `CF_OEMTEXT` from
//!   `CF_UNICODETEXT`, `CF_DIB` from `CF_BITMAP` and back, `CF_LOCALE` — and
//!   `EnumClipboardFormats` lists the synthesised ones alongside the real
//!   ones with nothing to tell them apart. A capture is therefore of the
//!   clipboard, not of the application, and the sidecar's `how` field is where
//!   that distinction gets recorded.

use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, HANDLE};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW,
    GetClipboardOwner, OpenClipboard,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use rclip_core::flavor::{cf, WindowsFormat};

use crate::capture::{bail, Body, Capture, Offered, Result, Selection};

/// Another process holds the clipboard open more often than one would like —
/// shell extensions and clipboard managers both do it — and the documented
/// answer is to retry rather than to fail.
const OPEN_ATTEMPTS: u32 = 10;
const OPEN_RETRY: std::time::Duration = std::time::Duration::from_millis(20);

/// Closes the clipboard however the function below exits.
struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: only constructed after a successful `OpenClipboard`, and it
        // is not `Clone`, so this runs exactly once per open.
        unsafe { CloseClipboard() };
    }
}

/// Unlocks an `HGLOBAL` however the borrow of it ends.
struct GlobalGuard(HANDLE);

impl Drop for GlobalGuard {
    fn drop(&mut self) {
        // SAFETY: only constructed after a successful `GlobalLock` of this
        // same handle, and the lock count is per-handle, so this balances it.
        unsafe { GlobalUnlock(self.0) };
    }
}

pub fn capture(_selection: Selection) -> Result<Capture> {
    // `Selection::Primary` is rejected in `main`: Windows has one clipboard.
    let _guard = open_clipboard()?;

    let owner = owner_process_name();
    let mut offered = Vec::new();

    // `EnumClipboardFormats(0)` starts the walk and each call returns the
    // format after the one it was given; 0 back means the end (or an error,
    // which `GetLastError` separates from a clean finish).
    let mut format = 0u32;
    loop {
        // SAFETY: the clipboard is open for the lifetime of `_guard`, which is
        // what this call requires.
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            // SAFETY: no preconditions.
            let err = unsafe { GetLastError() };
            if err != 0 {
                bail!("EnumClipboardFormats failed: Win32 error {err}");
            }
            break;
        }
        offered.push(read_format(format));
    }

    Ok(Capture {
        platform: rclip_core::Platform::Windows,
        source: match owner {
            Some(name) => format!("the Windows clipboard, owned by {name}"),
            None => "the Windows clipboard".to_owned(),
        },
        offered,
    })
}

fn open_clipboard() -> Result<ClipboardGuard> {
    for attempt in 0..OPEN_ATTEMPTS {
        // SAFETY: a null `HWND` is documented as "associate with the current
        // task", which is what a CLI with no window wants.
        if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
            return Ok(ClipboardGuard);
        }
        // SAFETY: no preconditions.
        let err = unsafe { GetLastError() };
        if err != ERROR_ACCESS_DENIED || attempt + 1 == OPEN_ATTEMPTS {
            bail!("OpenClipboard failed: Win32 error {err}");
        }
        std::thread::sleep(OPEN_RETRY);
    }
    unreachable!("loop either returns or bails")
}

/// One format: its name, and its bytes if it has any.
fn read_format(format: u32) -> Offered {
    let native = format_name(format);

    if let Some(why) = not_byte_addressable(format) {
        return Offered {
            native,
            item: None,
            body: Body::Skipped(why.to_owned()),
            detail: None,
        };
    }

    // SAFETY: the clipboard is open. The returned handle belongs to the
    // clipboard and must not be freed; it stays valid until `CloseClipboard`.
    let handle = unsafe { GetClipboardData(format) };
    if handle.is_null() {
        // SAFETY: no preconditions.
        let err = unsafe { GetLastError() };
        return Offered {
            native,
            item: None,
            body: Body::Skipped(format!(
                "GetClipboardData returned NULL (Win32 error {err}); the owner \
                 may have declined to render this format"
            )),
            detail: None,
        };
    }

    // SAFETY: `handle` is a non-null clipboard `HGLOBAL` for a format that
    // `not_byte_addressable` has cleared as memory rather than a GDI object.
    let ptr = unsafe { GlobalLock(handle) };
    if ptr.is_null() {
        // SAFETY: no preconditions.
        let err = unsafe { GetLastError() };
        return Offered {
            native,
            item: None,
            body: Body::Skipped(format!("GlobalLock failed: Win32 error {err}")),
            detail: None,
        };
    }
    let _unlock = GlobalGuard(handle);

    // SAFETY: `handle` is a valid `HGLOBAL`.
    let len = unsafe { GlobalSize(handle) };

    // SAFETY: `GlobalLock` returned a pointer to at least `GlobalSize` bytes,
    // valid until the matching `GlobalUnlock` in `_unlock`'s `Drop`. The bytes
    // are copied out before then. `u8` has no alignment requirement.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) }.to_vec();

    Offered {
        native,
        item: None,
        body: Body::Bytes(bytes),
        // GlobalSize reports the size of the *allocation*, which the docs
        // allow to be larger than what was asked for. For CF_UNICODETEXT the
        // NUL terminator is the authoritative end, not this length.
        detail: Some(format!(
            "length is GlobalSize ({len}), which may round up past the data"
        )),
    }
}

/// The canonical name of a format number.
///
/// Registered formats know their own name; predefined ones do not, and the
/// spellings used for them here are the ones `Flavor::from_windows_name`
/// recognises, so a dumped name round-trips back to the right `Flavor`.
fn format_name(format: u32) -> String {
    if let Some(name) = WindowsFormat::Predefined(format).name() {
        return name.to_owned();
    }
    if let Some(name) = predefined_name(format) {
        return name.to_owned();
    }

    // Registered names are capped at 255 characters + NUL by the API.
    let mut buf = [0u16; 256];
    // SAFETY: `buf` is writable for `buf.len()` u16s, which is what the third
    // argument promises. Fails (returns 0) for predefined formats, which is
    // why they are handled above.
    let len = unsafe { GetClipboardFormatNameW(format, buf.as_mut_ptr(), buf.len() as i32) };
    if len > 0 {
        return String::from_utf16_lossy(&buf[..len as usize]);
    }

    // An unnamed, unrecognised number. The ranges are worth spelling out: a
    // capture that says `CF_PRIVATEFIRST+3` is a capture that can be explained.
    match format {
        0x0200..=0x02FF => format!("CF_PRIVATEFIRST+{}", format - 0x0200),
        0x0300..=0x03FF => format!("CF_GDIOBJFIRST+{}", format - 0x0300),
        _ => format!("CF_{format}"),
    }
}

/// `CF_*` names `rclip-core` does not carry.
///
/// Every one of these resolves to `Flavor::Other`, so the name is only for the
/// human reading the table — but "CF_LOCALE" is a great deal more useful in a
/// capture than "CF_16".
const fn predefined_name(format: u32) -> Option<&'static str> {
    Some(match format {
        cf::METAFILEPICT => "CF_METAFILEPICT",
        cf::SYLK => "CF_SYLK",
        cf::DIF => "CF_DIF",
        cf::PALETTE => "CF_PALETTE",
        cf::PENDATA => "CF_PENDATA",
        cf::RIFF => "CF_RIFF",
        cf::WAVE => "CF_WAVE",
        cf::ENHMETAFILE => "CF_ENHMETAFILE",
        cf::LOCALE => "CF_LOCALE",
        0x0080 => "CF_OWNERDISPLAY",
        0x0081 => "CF_DSPTEXT",
        0x0082 => "CF_DSPBITMAP",
        0x0083 => "CF_DSPMETAFILEPICT",
        0x008E => "CF_DSPENHMETAFILE",
        _ => return None,
    })
}

/// Why `GlobalLock` must not be called on this format, if it must not.
///
/// `GetClipboardData` returns a handle whose *type* depends on the format, and
/// only the `HGLOBAL` ones are memory. Passing an `HBITMAP` to `GlobalLock` is
/// undefined behaviour, not an error return, so this check has to happen
/// before the call and not after it.
const fn not_byte_addressable(format: u32) -> Option<&'static str> {
    Some(match format {
        cf::BITMAP => "HBITMAP, not memory: needs GetDIBits and an alpha decision (rclip-dib)",
        cf::PALETTE => "HPALETTE, not memory",
        cf::ENHMETAFILE => "HENHMETAFILE, not memory: needs GetEnhMetaFileBits",
        cf::METAFILEPICT => "HGLOBAL holding a METAFILEPICT whose mfp.hMF is itself a handle",
        0x0080 => "CF_OWNERDISPLAY: the owner draws it, there is no data",
        0x0082 => "CF_DSPBITMAP: HBITMAP, not memory",
        0x0083 => "CF_DSPMETAFILEPICT: METAFILEPICT, whose hMF is a handle",
        0x008E => "CF_DSPENHMETAFILE: HENHMETAFILE, not memory",
        0x0300..=0x03FF => "CF_GDIOBJFIRST range: a GDI object handle, not memory",
        _ => return None,
    })
}

/// The image name of the process that owns the clipboard, best effort.
///
/// Every step is allowed to fail — the owner may have exited, may be elevated,
/// or may have set the clipboard without a window — and a failure just means
/// the sidecar says less. Nothing here is load-bearing.
fn owner_process_name() -> Option<String> {
    // SAFETY: no preconditions; returns NULL when the owner has no window.
    let hwnd = unsafe { GetClipboardOwner() };
    if hwnd.is_null() {
        return None;
    }

    let mut pid = 0u32;
    // SAFETY: `hwnd` is non-null and `pid` is a valid, writable `u32`.
    unsafe { GetWindowThreadProcessId(hwnd, &raw mut pid) };
    if pid == 0 {
        return None;
    }

    // SAFETY: no preconditions. LIMITED_INFORMATION is the least privilege
    // that still allows QueryFullProcessImageNameW.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let _close = ProcessGuard(process);

    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    // SAFETY: `process` is a live handle with the required access right, `buf`
    // is writable for `len` u16s, and `len` is a valid in/out `u32`.
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &raw mut len)
    };
    if ok == 0 {
        return None;
    }

    let full = String::from_utf16_lossy(&buf[..len as usize]);
    // The leaf name only: a full path is a home directory waiting to be
    // committed to a public corpus.
    Some(full.rsplit(['\\', '/']).next().unwrap_or(&full).to_owned())
}

struct ProcessGuard(HANDLE);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // SAFETY: only constructed from a successful `OpenProcess`, and not
        // `Clone`, so the handle is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

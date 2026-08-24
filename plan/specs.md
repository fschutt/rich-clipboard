# Spec reference index

Everything cited by [`PLAN.md`](PLAN.md). Grouped by platform; ★ = primary normative source
for a crate in this workspace.

## Windows

| Topic | Source |
|---|---|
| ★ Shell Link (.LNK) binary format | [MS-SHLLINK](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/16cb4ca1-9339-4d0c-a68d-bf1d6cc0f943) — rev 10.0, 2025-11-21. [PDF](https://winprotocoldocs-bhdugrdyduf5h2e4.b02.azurefd.net/MS-SHLLINK/%5bMS-SHLLINK%5d.pdf) |
| ↳ ExtraData blocks (§2.5) | [ExtraData](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/c41e062d-f764-4f13-bd4f-ea812ab9a4d1) |
| ↳ Serialized property storage | [MS-PROPSTORE](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-propstore/39ea873f-7af5-44dd-92f9-bc1f293852cc) |
| ↳ FILETIME, GUID packet repr | [MS-DTYP](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dtyp/cca27429-5689-4a16-b2b4-9325d93e4ba2) |
| ★ CF_HTML "HTML Format" | [HTML Clipboard Format](https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format) |
| ★ Shell clipboard formats (CFSTR_*) | [Shell Clipboard Formats](https://learn.microsoft.com/en-us/windows/win32/shell/clipboard) |
| Standard formats (CF_*) | [Standard Clipboard Formats](https://learn.microsoft.com/en-us/windows/win32/dataxchg/standard-clipboard-formats) |
| ★ BITMAPV5HEADER (CF_DIBV5) | [ns-wingdi-bitmapv5header](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmapv5header) |
| DROPFILES (CF_HDROP) | [ns-shlobj_core-dropfiles](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-dropfiles) |
| FILEDESCRIPTORW | [ns-shlobj_core-filedescriptorw](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-filedescriptorw) |
| CIDA (CFSTR_SHELLIDLIST) | [ns-shlobj_core-cida](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-cida) |
| ITEMIDLIST / SHITEMID | [ns-shtypes-itemidlist](https://learn.microsoft.com/en-us/windows/win32/api/shtypes/ns-shtypes-itemidlist) |
| DROPEFFECT constants | [DROPEFFECT](https://learn.microsoft.com/en-us/windows/win32/com/dropeffect-constants) |
| Virtual-file drag (background) | [The Old New Thing — dragging a virtual file](https://devblogs.microsoft.com/oldnewthing/20080318-00/?p=23083) |
| .url InternetShortcut (unofficial) | [cyanwerks URL file format](https://www.cyanwerks.com/formats/file-format-url.html) |
| PIDL internals (reverse-engineered) | ShellBags forensics literature; [libfwsi format docs](https://github.com/libyal/libfwsi/tree/main/documentation) |

**Note:** PIDL *contents* are not specified by Microsoft. `libfwsi`'s "Windows Shell Item format"
document is the best available reference and is what the forensics tooling agrees on.

## macOS

| Topic | Source |
|---|---|
| ★ BookmarkData / alias record | [mac_alias — Bookmark format](https://mac-alias.readthedocs.io/en/latest/bookmark_fmt.html) · [Alias format](https://mac-alias.readthedocs.io/en/latest/alias_fmt.html) · [source](https://github.com/dmgbuild/mac_alias) |
| ↳ independent RE writeups | [mikeymikey — BookmarkData exposed](http://michaellynn.github.io/2015/10/24/apples-bookmarkdata-exposed/) · [Mother's Ruin — URL bookmarks & security scoping](https://www.mothersruin.com/software/Archaeology/reverse/bookmarks.html) |
| NSPasteboard types | [NSPasteboard.PasteboardType](https://developer.apple.com/documentation/appkit/nspasteboard/pasteboardtype) |
| Uniform Type Identifiers | [UniformTypeIdentifiers framework](https://developer.apple.com/documentation/uniformtypeidentifiers) |
| Clipboard etiquette / transient data | [nspasteboard.org](https://nspasteboard.org/) |
| .webloc / .inetloc / .textClipping | [Eclectic Light — data formats in textClipping, webloc, mailloc](https://eclecticlight.co/2025/12/30/data-formats-used-in-textclipping-webloc-and-mailloc-files-1994-2025/) |
| Finder aliases vs bookmarks | [Eclectic Light — Finder aliases and bookmarks](https://eclecticlight.co/2019/01/12/finder-aliases-and-bookmarks-a-summary/) |

## X11

| Topic | Source |
|---|---|
| ★ Selections, INCR, targets | [ICCCM §2](https://www.x.org/releases/X11R7.6/doc/xorg-docs/specs/ICCCM/icccm.html#peer_to_peer_communication_by_means_of_selections) |
| ★ XDND drag-and-drop | [XDND spec](https://johnlindal.wixsite.com/xdnd) — v5, 2010-07-30 |
| Clipboard manager (SAVE_TARGETS) | [freedesktop clipboards spec](https://www.freedesktop.org/wiki/ClipboardManager/) |
| text/uri-list | [RFC 2483 §5](https://www.rfc-editor.org/rfc/rfc2483#section-5) |
| Practical clipboard survey | [indigo.re — Clipboard data](https://indigo.re/posts/2021-12-21-clipboard-data.html) |

**XDND essentials** (from the spec, for the transport layer in azul):
`XdndAware` property on the target window holds the highest supported version.
`XdndEnter` `data.l[0]`=source window, `l[1]`= bit0 "more than 3 types" + version in high byte,
`l[2..4]`= first three type atoms; more types come from the `XdndTypeList` property.
`XdndPosition` `l[2]`=`(x<<16)|y`, `l[3]`=timestamp, `l[4]`=requested action.
`XdndStatus` `l[1]` bit0=accept, `l[2..3]`=no-update rect, `l[4]`=accepted action.
`XdndDrop` `l[2]`=timestamp; data then arrives via the `XdndSelection` selection.
`XdndFinished` (v2+) `l[1]` bit0=success, `l[2]`=action performed.
Actions: `XdndActionCopy` (default), `Move`, `Link`, `Ask`, `Private`.
Version = min(source, target). Types are lowercase MIME atoms; `text/plain` without a charset
parameter means ISO-8859-1, and `;charset=` is v4+.

**Direct save (promised files):** `XdndDirectSave0` (XDS) — the X11 analogue of
`CFSTR_FILECONTENTS` / `NSFilePromiseProvider`.

## Wayland

| Topic | Source |
|---|---|
| ★ wl_data_device / _source / _offer | [Wayland protocol spec, appendix A](https://wayland.freedesktop.org/docs/html/apa.html) |
| Clipboard + DnD walkthrough | [emersion — Wayland clipboard and drag & drop](https://emersion.fr/blog/2020/wayland-clipboard-drag-and-drop/) |
| Primary selection | [wp-primary-selection-unstable-v1](https://wayland.app/protocols/primary-selection-unstable-v1) |
| Data control (clipboard managers) | [ext-data-control-v1](https://wayland.app/protocols/ext-data-control-v1) · [wlr-data-control-unstable-v1](https://wayland.app/protocols/wlr-data-control-unstable-v1) |

MIME types are identical to X11's; the difference is purely transport — negotiation is by MIME
string, and the payload arrives over a pipe fd that the source writes and closes.

## Cross-desktop (Linux)

| Topic | Source |
|---|---|
| ★ Desktop Entry Spec (`.desktop`, `Type=Link`) | [v1.5](https://specifications.freedesktop.org/desktop-entry/latest/) |
| Existing Rust parsers to evaluate | [`freedesktop_entry_parser`](https://docs.rs/freedesktop_entry_parser) · [`freedesktop-file-parser`](https://crates.io/crates/freedesktop-file-parser) |

Undocumented but load-bearing conventions:
- `x-special/gnome-copied-files` — first line `copy` or `cut`, then `file://` URIs.
- `x-special/nautilus-clipboard` — Nautilus-specific variant.
- `application/x-kde-cutselection` — `"1"` = cut.

## Format-agnostic

| Topic | Source |
|---|---|
| RTF 1.9.1 spec | Microsoft "Rich Text Format (RTF) Specification, Version 1.9.1" (Office file-format downloads) |
| Existing Rust .lnk crates (prior art) | [`lnk`](https://crates.io/crates/lnk) (lilopkins, v0.6.4 — name taken) · [`parselnk`](https://docs.rs/parselnk) · [`lnk_parser`](https://lib.rs/crates/lnk_parser) |

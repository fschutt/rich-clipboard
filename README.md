# rich-clipboard

Parsers and serializers for the byte formats behind clipboard and drag-and-drop on Windows,
macOS, X11 and Wayland — built for the [azul](https://github.com/fschutt/azul) GUI framework, but
free of any OS dependency.

Drop a `.lnk` on an app and get a parsed shell link instead of a filename. Paste from Word and
get styled runs instead of flattened text. Copy an image and have the alpha channel survive.

**No OS calls live here.** Transport — `IDataObject`, `NSPasteboard`, ICCCM selections,
`wl_data_offer` — stays in the framework. This workspace is `&[u8] → T` and back, which is what
makes a Windows `.lnk` parser testable on a Mac.

Every codec is `#![no_std]`, `#![forbid(unsafe_code)]`, borrows rather than allocates, and treats
its input as hostile — because a clipboard payload is written by another process and parsed the
instant a user presses Ctrl+V.

- [`plan/PLAN.md`](plan/PLAN.md) — the design plan and phasing
- [`plan/specs.md`](plan/specs.md) — every spec cited, with links
- [`plan/CONVENTIONS.md`](plan/CONVENTIONS.md) — rules for codec crates
- [`plan/INTEGRATION.md`](plan/INTEGRATION.md) — **wiring this into a GUI framework**: the seam, the API, and the per-platform traps

## Status

Phase 0 complete. Twelve crates, 416 tests, and a synthetic corpus with sidecars for every
format. Every codec builds for `thumbv7em-none-eabi`, forbids `unsafe`, and takes **no
dependency beyond `rclip-core`** — that last one was not a rule, it is what seven independent
prior-art reviews concluded.

| Crate | Format | Platform |
|---|---|---|
| `rclip-core` | Flavor registry, bounds-checked reader, errors | all |
| `rclip-cf-html` | CF_HTML "HTML Format" | win |
| `rclip-rtf` | RTF 1.9.1, clipboard subset | win, mac |
| `rclip-dib` | CF_DIB / CF_DIBV5 | win |
| `rclip-dropfiles` | CF_HDROP / DROPFILES | win |
| `rclip-file-desc` | FILEGROUPDESCRIPTORW, virtual files | win |
| `rclip-idlist` | ITEMIDLIST / PIDL / CIDA | win |
| `rclip-shell-link` | `.lnk` (MS-SHLLINK) | win |
| `rclip-url-file` | `.url` InternetShortcut | win |
| `rclip-bookmark` | macOS BookmarkData / alias | mac |
| `rclip-webloc` | `.webloc` / `.inetloc` | mac |
| `rclip-uri-list` | text/uri-list + GNOME/KDE cut conventions | x11, wl |
| `rclip-desktop-entry` | freedesktop `.desktop`, incl. `Type=Link` | x11, wl |

Next: real captures from real applications (`corpus/<platform>/`), `cargo-fuzz` targets, the
`rich-clipboard` facade, and wiring into azul.

## License

MIT OR Apache-2.0

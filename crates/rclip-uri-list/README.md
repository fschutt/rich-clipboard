# rclip-uri-list

`text/uri-list` — and the undocumented conventions that make a **cut** survive a paste on Linux.

Two layers, because in practice they are never used apart:

- **[RFC 2483 §5](https://www.rfc-editor.org/rfc/rfc2483#section-5)** — CRLF-separated URIs, lines
  beginning `#` are comments, everything percent-encoded. Four sentences of specification.
- **The cut-vs-copy conventions**, which are specified nowhere. Without one of them, every paste of
  files reads as a copy and "cut" silently becomes "duplicate".

`#![no_std]`, `#![forbid(unsafe_code)]`, parsing borrows from the caller's buffer. Percent
*validation* and byte-wise decoding need no allocator; producing a decoded `String` and the
serializers are behind the `alloc` feature.

## The conventions, and where each grammar was read from

There is no spec for any of these, so `src/convention.rs` cites the implementation instead.

| Payload | Grammar | Source |
|---|---|---|
| `x-special/gnome-copied-files` | `verb *( LF uri )`, verb is `copy`/`cut`, **LF only, no trailing newline** | Nautilus `src/nautilus-clipboard.c` |
| `x-special/mate-copied-files` | identical | Caja `libcaja-private/caja-clipboard-monitor.c` |
| `application/x-kde-cutselection` | one ASCII byte, `1` = cut, `0` = copy | KIO `src/widgets/paste.cpp` |
| `x-special/nautilus-clipboard` | magic first line inside a `text/plain` payload, `magic LF verb LF uri…LF` | Nautilus 3.38 `src/nautilus-clipboard.c` |

Four things the folklore gets wrong, all checked against current source:

- **`x-special/nautilus-clipboard` was never a MIME type.** It was a magic first line inside the
  `text/plain` payload between Nautilus 3.30 and 3.38, and Nautilus stopped writing it in version 40
  (commit `2045f662`, 2021-03) — not 42 or 43. This crate still *reads* it, because old payloads
  outlive the code that made them, but `emit::RECOMMENDED` does not offer it.
- **`x-special/gnome-copied-files` has no trailing newline, and that is load-bearing.** Since
  Nautilus 44 (commit `ee5a3586`) the reader splits on `\n` and rejects the entire payload if any
  line is empty or fails `g_uri_is_valid`. A trailing newline, a CRLF, or an unencoded space is not
  degraded — the paste does nothing at all.
- **`application/x-kde-cutselection` carries `"0"` for copy.** It is not a cut-only marker, so
  "payload present" must not be read as "cut". KIO's reader inspects byte 0 only, and
  `parse_kde_cut_selection` reproduces that exactly.
- **KDE and GNOME do not interoperate.** KIO neither writes nor reads `x-special/gnome-copied-files`
  (checked across KIO, KCoreAddons' `KUrlMimeData`, Dolphin and Plasma); GNOME does not read
  `x-kde-cutselection`; Chromium implements neither and has no cut/copy distinction for Linux files
  at all. A source has to publish all of them — which is what `emit::RECOMMENDED` lists.

`x-special/KDE-copied-files`, which appears in some third-party code, is not defined by any KDE
source and is not supported here.

## RFC 2483 vs. observed reality

The spec says CRLF. Real readers are laxer, and this one matches them:

- GLib's `g_uri_list_extract_uris` (what GTK deserializes with) documents that it "allow[s] LF
  delimination as well as the specified CRLF", trims whitespace off the ends, and does not validate
  URIs. Qt's `QMimeData::dataToUrls` splits on `'\n'` and calls `.trimmed()`. Chromium hands
  `"\r\n"` to `SplitStringPiece`, which treats it as a *character set*. So this crate splits on
  CRLF, LF or a lone CR, and trims.
- Qt's `qmimedata.cpp` carries a comment about Qt 3 appending a **trailing NUL** to this type and
  no other; one is stripped.
- Qt's reader does *not* skip `#` comment lines — a divergence from GTK and Chromium, and from
  RFC 2483. This crate skips them.

## Prior art

- **No crate implements this format.** Searched crates.io for `uri-list`, `uri_list`, `rfc2483` and
  `text/uri-list`: the string appears only *inside* clipboard/DnD integration crates (`clipawl`,
  `lamco-clipboard-core`, `hjkl-clipboard`, `dioxus-dnd`), none of which exposes a reusable parser
  and all of which are heavy platform-integration crates pulling a windowing stack.
- Nothing at all covers the GNOME/KDE cut-vs-copy conventions, which are the part with actual
  content. Written from scratch.

## Not implemented yet

- `// TODO(phase-4):` `ShortcutTarget` is a byte-identical mirror of the definition in
  `rclip-url-file`; Phase 4 hoists one copy into `rclip-core` and deletes the mirrors. Codec crates
  in this workspace do not depend on each other, so it is duplicated rather than imported.
- Percent-*encoding* (the write direction). `emit::write_uri_list` passes URIs through verbatim,
  because this crate cannot know whether a given byte in a caller's path was meant literally.
- `application/vnd.portal.filetransfer`, the XDG portal handle that Chromium and GTK now prefer for
  cross-sandbox file transfer. It is a D-Bus interaction, not a byte format, so it belongs to the
  transport layer rather than here.

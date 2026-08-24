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

## Percent-encoding, both directions

`Uri::percent_decode` reads; `uri::percent_encode` writes. Both work with no allocator — the
encoder is an iterator of ASCII bytes that also implements `Display`, so a `no_std` caller can
write straight into a `fmt::Write`. `emit::file_uri` is the one a clipboard source actually
wants: a path in, a `file://` URI out.

The set of characters left literal is the whole difficulty, and it breaks in **both** directions:

| | What it looks like | What it costs |
|---|---|---|
| Under-encoding | `file:///tmp/notes#2.txt` | `#` starts a fragment, so the file arrives as `/tmp/notes` |
| Under-encoding | `file:///tmp/100%.txt` | the reader takes `%.t` for an escape |
| Over-encoding | `file:///tmp%2Fa.txt` | `%2F` is not a separator; one path becomes one long segment |
| Over-encoding | `file:///tmp/it%27s.txt` | RFC 3986 §6.2.2.2: an encoded *reserved* character is not equivalent to its literal form, so a reader comparing URIs stops matching one it produced itself |

`EncodeSet::Path` is RFC 3986's `pchar` (`unreserved / sub-delims / ":" / "@"`) plus the `/`
separator — literally `A-Za-z0-9-._~!$&'()*+,;=:@/`. That is byte-for-byte GLib's
`G_URI_RESERVED_CHARS_ALLOWED_IN_PATH`, which is what `g_filename_to_uri` escapes with, so a URI
built here is textually identical to the one Nautilus would have built for the same path. Hex
digits are uppercase, per §2.1 and per what GLib, Qt and Chromium all emit.

Two smaller decisions worth knowing:

- The encoder takes **bytes**, not `&str`, for the same reason the decoder yields them: a POSIX
  path is a byte string and the ones that are not UTF-8 are exactly the ones a caller most needs
  to move rather than reject.
- `file_uri` supplies a leading `/` if the path lacks one, because `file://home/me` parses `home`
  as an *authority* — a hostname — not as the first path component. That is the one malformation
  that changes what the URI means rather than how it looks. It is a syntactic category error, not
  path resolution, which this crate still does not do.

`emit::write_uri_list` continues to write URIs through **verbatim**, and that is not a gap: a
`&str` handed to it is already a URI, and re-encoding one doubles every `%` it contains
(`a%20b` becomes `a%2520b`). Paths go through `file_uri` first.

## Prior art

- **No crate implements this format.** Searched crates.io for `uri-list`, `uri_list`, `rfc2483` and
  `text/uri-list`: the string appears only *inside* clipboard/DnD integration crates (`clipawl`,
  `lamco-clipboard-core`, `hjkl-clipboard`, `dioxus-dnd`), none of which exposes a reusable parser
  and all of which are heavy platform-integration crates pulling a windowing stack.
- Nothing at all covers the GNOME/KDE cut-vs-copy conventions, which are the part with actual
  content. Written from scratch.

## The shared shortcut type

Each entry's destination comes back as `ShortcutTarget<'a>`, re-exported from
`rclip_core::shortcut` and shared with `rclip-url-file`, `rclip-webloc`, `rclip-shell-link` and
`rclip-desktop-entry` — see `plan/PLAN.md` §4.10.

## Not implemented yet

- `application/vnd.portal.filetransfer`, the XDG portal handle that Chromium and GTK now prefer for
  cross-sandbox file transfer. It is a D-Bus interaction, not a byte format, so it belongs to the
  transport layer rather than here.

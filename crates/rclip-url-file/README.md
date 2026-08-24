# rclip-url-file

Parser for Windows `.url` — the `[InternetShortcut]` file.

A `.url` is what Explorer writes when you drag a browser's address bar onto the desktop, and what
lands on the clipboard as a file when you copy one. INI-shaped text; the only required key is
`[InternetShortcut] URL=`.

`#![no_std]`, `#![forbid(unsafe_code)]`, parsing borrows from the caller's buffer and allocates
nothing.

## There is no specification — treat everything here as observed

Microsoft never published one. This crate is written against two unofficial sources:

- **[An Unofficial Guide to the URL File Format](https://www.cyanwerks.com/formats/file-format-url.html)**
  (Edward L. Blake, 3rd ed.) — the key list, the `HotKey` number table, and the `Modified` FILETIME
  encoding. Explicitly unofficial; its own disclaimer says so.
- **Wine's `dlls/ieframe/intshcut.c`** — an actual reimplementation of `IUniformResourceLocator` /
  `IPersistFile`. This is where the parser's case-insensitivity, CRLF and UTF-8 behaviour come from.

Every field description in the source is marked as observed rather than specified.

## What Win32's profile API actually does, and why it matters

`.url` is not a format so much as "whatever `GetPrivateProfileString` accepts". `src/ini.rs`
reproduces the parts a real file depends on:

- **Section and key names compare ASCII-case-insensitively.** Wine's `Save` writes `ICONFILE=` /
  `ICONINDEX=` and its `Load` reads them back as `iconfile` / `iconindex`; that only round-trips
  because the profile API folds case. A case-sensitive parser silently loses the icon on files
  written by Wine and by several installers.
- Whitespace around `=` and around the value is stripped, and **one matched pair of double quotes**
  is removed — installers rely on that to keep a trailing space.
- **`;` starts a comment; `#` does not.** The profile API only knows `;`.
- The **first** occurrence of a key wins.

`HotKey=` is decoded into a virtual-key code plus `HOTKEYF_*` modifier bits — the same encoding as
`ShellLinkHeader.HotKey` in MS-SHLLINK, which is what lets a `.url` and a `.lnk` agree on a shortcut
key. Verified against the cyanwerks table: `1601` is `0x0641`, `'A'` with `CONTROL|ALT`, which that
table lists as Ctrl+Alt+A.

`Modified=` is eight little-endian `FILETIME` bytes as hex, plus a ninth byte the guide calls a
checksum and this crate hands back raw.

## `[InternetShortcut.A]` and `[InternetShortcut.W]`

Returned **verbatim and undecoded**, because nobody has documented what they contain. The NSIS
wiki's own annotation is `[InternetShortcut.A] ; CP_ACP stuff?` / `[InternetShortcut.W] ; UTF-7
stuff?` — question marks in the original. Guessing an encoding here would silently corrupt a URL, so
`url_ansi()` and `url_wide()` hand back the bytes as written and say so.

## Prior art

**Nothing exists.** Searched crates.io for `InternetShortcut`, "internet shortcut", "url file
parser", "windows shortcut url", "ini windows url file". Everything in that space targets the
*binary* `.lnk` Shell Link format instead — `lnk`, `parselnk`, `lnk_parser`, `lnk-core`, `mslnk`,
`shortcuts-rs` — which is a different format entirely. `win-desktop-utils` *creates* shortcuts
through the shell API on Windows; it is not a parser and not portable. **Wrote our own**; it is an
INI-ish file and the interesting parts are the Win32 profile-API behaviours above, which no generic
INI crate reproduces.

## The shared shortcut type

`ShortcutTarget<'a>` — `Url` / `Path` / `Unresolved` — lives here. `.url`, `.webloc`, `.desktop`
(`Type=Link`) and macOS `BookmarkData` are four spellings of one idea, and `plan/PLAN.md` §4.10
unifies them in Phase 4; this crate is the family's smallest member, so nothing has to depend on
anything larger to get at the type. `rclip-desktop-entry` and `rclip-uri-list` carry byte-identical
mirrors, because codec crates in this workspace do not depend on each other.

`// TODO(phase-4):` hoist it into `rclip-core` and delete the mirrors.

Classification order is the whole correctness of `ShortcutTarget::classify`: `C:\Users\me` is a
*syntactically valid* RFC 3986 reference with scheme `C`, so the drive-letter and UNC tests run
before the scheme test. Otherwise every Windows path on the clipboard becomes a URL with a
one-letter scheme.

## Not implemented yet

- `// TODO(phase-3):` `IDList=` is returned as written. Decoding it needs a PIDL parser; it will go
  through `rclip-idlist` once that crate exists. In files on disk it is almost always empty.
- Code-page transcoding. `parse()` requires UTF-8: real files are ASCII (a `URL=` is
  percent-encoded) and Wine writes UTF-8, but a legacy file in a Windows code page must be
  transcoded by the caller. A `// TODO(phase-4):` if the Phase-1 corpus turns up one that matters.
- The `[DEFAULT]` / `[DOC#n]` `BASEURL`/`ORIGURL` frameset sections, and the extended keys NSIS
  lists (`Roamed`, `Author`, `WhatsNew`, `Desc`, `FeedUrl`, `IsLivePreview`, `PreviewSize`). They
  parse as ordinary sections and entries — `sections()` and `Section::get` reach them — but have no
  typed accessors, because nothing in the plan reads them.
- No serializer. Nothing in `plan/PLAN.md` needs to write a `.url` before Phase 4.

# rich-clipboard — plan

Parsing/serialization primitives for **rich clipboard and drag-and-drop payloads**, to be
consumed by [azul](../../azul). This repo contains **no OS API calls**. It turns the byte blobs
that Win32 / NSPasteboard / X11 selections / Wayland fds hand you into typed Rust data, and
back again.

---

## 1. Scope boundary

Clipboard/DnD splits into three layers. Only layer 2 lives here.

| Layer | What it is | Where it lives |
|---|---|---|
| **1. Transport** | `OpenClipboard`/`IDataObject`, `NSPasteboard`, ICCCM selections + INCR, `wl_data_offer` fd reads, XDND messages | stays in `azul/dll/src/desktop/shell2/*/clipboard.rs` |
| **2. Codecs** ← *this repo* | `&[u8] -> T`, `T -> Vec<u8>`. No `unsafe`, no syscalls, no I/O, `no_std + alloc` where possible | `rich-clipboard/*` |
| **3. Policy** | which flavor to prefer, lossy conversions between flavors, azul's `ClipboardContent` | thin facade crate here + azul |

Why the split matters: layer 2 is 90% of the work, 100% of the security surface, and the only
part that is testable without a display server. Keeping it OS-free means the Windows `.lnk`
parser is unit-tested on the macOS dev machine.

## 2. The seam in azul today

Azul's clipboard is **plain text only**, on all four backends:

- `azul/dll/src/desktop/shell2/common/event.rs:273` — `fn get_system_clipboard() -> Option<String>`
- `azul/dll/src/desktop/shell2/common/event.rs:298` — `fn set_system_clipboard(text: String) -> bool`

Both fan out to per-platform modules. Windows is 24 lines of `clipboard-win` `formats::Unicode`;
macOS is `NSPasteboard writeObjects:[NSString]`; X11 is a real ICCCM implementation (552 lines)
but only ever negotiates `UTF8_STRING`; Wayland reads one mime type.

`ClipboardContent` (`azul/layout/src/managers/selection.rs:48`) already has the right *shape* for
rich text — `plain_text: AzString` + `styled_runs: StyledTextRunVec` — and already knows how to
emit HTML. It has no representation for images, file lists, or links.

**Integration target:** replace those two functions with

```rust
fn get_system_clipboard() -> Option<ClipboardPayload>;   // all offered flavors, undecoded
fn set_system_clipboard(p: ClipboardPayload) -> bool;    // all flavors we can synthesize
```

where `ClipboardPayload` is `Vec<(Flavor, Vec<u8>)>` from this repo. Everything else here is
codecs feeding that one type. Doing the type change first, with only the text flavor wired, is a
safe no-behavior-change refactor that unblocks all later work.

## 3. Workspace layout

Cargo workspace, one crate per format. Codecs never depend on each other except where noted.

```
rich-clipboard/
├── rclip-core/          flavor enum, format registry, byte-reader, error types
├── rclip-cf-html/       CF_HTML "HTML Format"                    [win]
├── rclip-rtf/           RTF 1.9.1 subset, read + write           [win, mac]
├── rclip-dib/           CF_DIB / CF_DIBV5 <-> RGBA8              [win]
├── rclip-dropfiles/     CF_HDROP / DROPFILES                     [win]
├── rclip-uri-list/      text/uri-list + gnome/kde cut conventions [x11, wl]
├── rclip-idlist/        ITEMIDLIST / PIDL / CIDA                 [win]
├── rclip-shell-link/    .lnk (MS-SHLLINK)     -> depends on idlist [win]
├── rclip-file-desc/     FILEGROUPDESCRIPTORW (virtual files)     [win]
├── rclip-url-file/      .url InternetShortcut (INI)              [win]
├── rclip-bookmark/      macOS BookmarkData / alias record        [mac]
├── rclip-webloc/        .webloc / .inetloc (plist)               [mac]
├── rclip-desktop-entry/ freedesktop .desktop, incl. Type=Link    [x11, wl]
├── rich-clipboard/      facade: re-exports + flavor<->type mapping
├── corpus/              real captured blobs, checked in
├── tools/               per-OS capture + oracle binaries
└── fuzz/                cargo-fuzz targets, one per parser
```

**Naming note:** `lnk` is taken on crates.io (lilopkins/lnk-rs, v0.6.4, 83k downloads). So are
most of the short generic names. Prefix published crates `rclip-*`; azul re-exports under its own
names if it wants nicer ones.

---

## 4. Per-crate specs

### 4.1 `rclip-core`

The vocabulary every other crate targets.

```rust
pub enum Flavor {
    PlainTextUtf8, Html, Rtf,
    ImagePng, ImageTiff, ImageDib, ImageDibV5,
    FileList, UriList, ShellIdList, ShellLink, FileDescriptor,
    Url, UrlName,
    DropEffect,
    Other(String),
}

/// Everything a source offered, still encoded.
pub struct ClipboardPayload { pub items: Vec<ClipboardItem> }
pub struct ClipboardItem { pub flavor: Flavor, pub bytes: Vec<u8> }
```

Plus the **format registry**: `Flavor` ⇄ platform identifier, and a read-preference order.
This table is the actual cross-platform knowledge and belongs in code, not prose:

| Flavor | Windows | macOS UTI | X11 / Wayland MIME |
|---|---|---|---|
| Plain text | `CF_UNICODETEXT` (13) | `public.utf8-plain-text` | `text/plain;charset=utf-8`, `UTF8_STRING` |
| HTML | `"HTML Format"` (registered) | `public.html` | `text/html` |
| RTF | `"Rich Text Format"` (registered) | `public.rtf`, `public.rtfd` | `text/rtf`, `application/rtf` |
| PNG | `"PNG"` (registered) | `public.png` | `image/png` |
| Raster (native) | `CF_DIBV5` (17), `CF_DIB` (8) | `public.tiff` | `image/bmp`, `image/tiff` |
| File list | `CF_HDROP` (15) | `public.file-url` × N items | `text/uri-list` |
| Cut-vs-copy | `CFSTR_PREFERREDDROPEFFECT` DWORD | — (Finder has no cut) | 1st line of `x-special/gnome-copied-files`; `application/x-kde-cutselection` |
| Namespace objs | `CFSTR_SHELLIDLIST` (CIDA) | — | — |
| Virtual files | `CFSTR_FILEDESCRIPTORW` + `CFSTR_FILECONTENTS` | `NSFilePromiseProvider` | XDND `XdndDirectSave0` (XDS) |
| URL | `CFSTR_INETURL` | `public.url` + `public.url-name` | `text/uri-list` |

Two traps to encode in the registry, not in each caller:

- **`text/html` encoding is unreliable.** Some producers write UTF-16 (with or without BOM),
  some UTF-8. Sniff BOM → try UTF-8 → fall back to UTF-16LE. Never assume.
- **macOS dynamic UTIs** (`dyn.ah62d4rv4ge8...`) are a decodable base-34 encoding of a legacy
  OSType/MIME/extension. Worth a ~100-line decoder so unknown flavors are still labelled.

Also here: a shared bounds-checked `Reader` (`u8/u16/u32/u64_le`, `take(n)`, `utf16_nul`,
`cstr_cp`) so no codec ever does raw slice indexing. Every parse error carries an offset.

### 4.2 `rclip-cf-html` — Windows rich text

Spec: [HTML Clipboard Format](https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format).
ASCII header, UTF-8 body, `\r\n` / `\n` / `\r` line ends:

```
Version:1.0
StartHTML:0000000121
EndHTML:0000000272
StartFragment:0000000180
EndFragment:0000000225
StartSelection:...      (optional, both or neither)
EndSelection:...
<html><!--StartFragment-->…<!--EndFragment--></html>
```

- Offsets are **byte offsets from the start of the whole blob**, and may be left-padded with
  arbitrary zeros. Trust the `<!--StartFragment-->` comments over the numbers when they disagree
  — real producers get the numbers wrong.
- `StartHTML`/`EndHTML` may be `-1` (fragment only, no context).
- Writing is the fiddly half: the offsets are self-referential. Emit fixed-width 10-digit
  placeholders, then back-patch. Do **not** iterate to a fixed point.
- `Version:0.9` pre-20H2, `Version:1.0` after.

API: `parse(&[u8]) -> CfHtml { version, context: Option<&str>, fragment: &str, selection: Option<&str>, source_url: Option<&str> }`
and `CfHtmlBuilder::new(fragment).source_url(..).build() -> Vec<u8>`.

### 4.3 `rclip-rtf` — the highest-value parser

This is the one that buys the most interop for the effort. On macOS, `public.rtf` is *the* rich
flavor — Pages, TextEdit, Mail and Notes all speak it and many speak no HTML. On Windows,
Word/Outlook offer RTF alongside CF_HTML and RTF is the higher-fidelity one.

Scope deliberately narrow — clipboard-grade styled text, not a word processor:

- Tokenizer: groups `{}`, control words `\word-123 `, control symbols `\*`, `\\`, `\{`.
- Destinations to read: `\fonttbl`, `\colortbl`, `\*\generator`.
- Destinations to **skip wholesale** (`{\*\...}` unknown-destination rule): everything else.
  Getting the skip rule right is what makes this robust against Word's output.
- Character properties: `\b \b0 \i \i0 \ul \ulnone \strike \fsN` (half-points!) `\cfN \cbN \fN`.
- Paragraph: `\par \pard \line \tab`.
- Text encoding: `\ansi` + `\ansicpgN`, `\uN?` (signed 16-bit, negative = wraparound) with
  `\ucN` skip-count, `\'hh` raw codepage bytes. The `\uc` skip counter is the single most
  commonly-botched part of RTF parsing — it must be tracked per group and restored on `}`.
- Writer: `StyledRun[] -> Vec<u8>` emitting a minimal font/color table. Always escape non-ASCII
  as `\uN?` with an ASCII fallback char; never emit raw high bytes.

Output type is the shared `RichText { runs: Vec<StyledRun>, plain: String }` so RTF, CF_HTML and
azul's `ClipboardContent` are mutually convertible through one hub.

### 4.4 `rclip-dib` — images without an image library

`CF_DIB` = `BITMAPINFOHEADER` + palette + pixels. `CF_DIBV5` = `BITMAPV5HEADER` + optional 3
DWORD bitfield masks + pixels. Header size field (first `u32`, 40 / 108 / 124) discriminates
`BITMAPINFOHEADER` / `BITMAPV4HEADER` / `BITMAPV5HEADER`.

Must handle: 1/4/8/16/24/32 bpp, palette lookup, `BI_RGB` vs `BI_BITFIELDS`, rows padded to
4-byte stride, and **negative `biHeight` = top-down** (positive = bottom-up, the default).

The alpha trap, and why this needs a policy flag rather than a guess: **`CF_DIBV5` has no agreed
alpha convention.** Chrome and Firefox write *premultiplied* RGBA; XnView and Photoshop assume
*straight*. There is no in-band signal. So:

```rust
pub enum AlphaMode { Straight, Premultiplied, Guess }
pub fn decode(bytes: &[u8], alpha: AlphaMode) -> Result<RgbaImage, DibError>;
```

with `Guess` documented as "heuristic: treat as premultiplied if any pixel has a channel > alpha
(impossible under premultiplication), else straight." Also: a `BITMAPINFOHEADER` with 32bpp
technically has *no* alpha channel — the 4th byte is undefined — so `CF_DIB` at 32bpp should
decode opaque unless every alpha byte is nonzero.

PNG/TIFF/JPEG are **not** decoded here — delegate to `image` or azul's existing decoders. This
crate owns only the two formats no one else implements.

### 4.5 `rclip-dropfiles` — CF_HDROP

```c
typedef struct { DWORD pFiles; POINT pt; BOOL fNC; BOOL fWide; } DROPFILES; // 20 bytes
```
`pFiles` is a byte offset (usually 20) to a double-NUL-terminated array of paths — UTF-16LE when
`fWide`, else system ANSI. `pt`/`fNC` carry the drop point for drag operations.

Read `Vec<PathBuf>` + drop point; write from `&[&Path]`. Trivial, but it is *the* way files enter
and leave a Windows app, and the double-NUL termination is a classic off-by-one.

### 4.6 `rclip-uri-list` — Linux file transfer, in practice

RFC 2483 `text/uri-list`: CRLF-separated URIs, lines starting `#` are comments, percent-encoded.

But the format that actually carries cut/copy on Linux is unspecified convention:

- **GNOME** `x-special/gnome-copied-files`: first line is literally `copy` or `cut`, then one
  `file:///…` URI per line. Nautilus also emits `x-special/nautilus-clipboard`.
- **KDE**: `text/uri-list` plus `application/x-kde-cutselection` containing `"1"` for cut.

Without these, a paste of files always reads as a copy. This crate should own the whole
convention set including which to emit for maximum compatibility (answer: emit all three).

### 4.7 `rclip-idlist` — PIDLs

`CIDA` (behind `CFSTR_SHELLIDLIST`) is documented:
```c
typedef struct { UINT cidl; UINT aoffset[1]; } CIDA;  // aoffset[0] = parent folder PIDL
```
The **contents** of each `ITEMIDLIST` are not. They are well reverse-engineered by the ShellBags
forensics community: `SHITEMID { u16 cb; u8 abID[cb-2] }`, terminated by a `u16` 0. Item classes
by first byte: `0x1F` root/GUID, `0x20-0x2F` volume/drive, `0x30-0x3F` file entry (with the
`0xBEEF0004` extension block carrying the long name), `0x40-0x4F` network, `0x60-0x6F` URI.

Parse **defensively and partially** — return `Unknown { class, raw }` for anything unrecognized
rather than failing. Most consumers only need "give me a display path if you can."

Separate crate because both `rclip-shell-link` (LinkTargetIDList, VistaAndAboveIDListDataBlock)
and the `CFSTR_SHELLIDLIST` handler need it.

### 4.8 `rclip-shell-link` — .lnk, from the existing template

Seed from https://github.com/fschutt/lnk. Current state: `ShellLinkHeader` parses; everything
else is types-with-doc-comments and `LinkTargetIdList::try_from` returns `Err(Unimplemented)`.
The doc comments transcribed from the spec are the valuable part — keep them.

Work to finish, per [MS-SHLLINK](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/16cb4ca1-9339-4d0c-a68d-bf1d6cc0f943) (rev 10.0, 2025-11-21):

1. `LinkTargetIDList` — delegate to `rclip-idlist`.
2. `LinkInfo` (§2.3) — `VolumeID`, `CommonNetworkRelativeLink`, the ANSI/Unicode offset pairs
   gated on `LinkInfoHeaderSize >= 0x24`.
3. `StringData` (§2.4) — 5 optional counted strings in fixed order: `NAME_STRING`,
   `RELATIVE_PATH`, `WORKING_DIR`, `COMMAND_LINE_ARGUMENTS`, `ICON_LOCATION`. Count is in
   *characters*, not bytes; `IsUnicode` in `LinkFlags` picks UTF-16 vs codepage.
4. `ExtraData` (§2.5) — 11 blocks, all signatures verified against the live spec:

   | Block | Signature | BlockSize |
   |---|---|---|
   | EnvironmentVariableDataBlock | `0xA0000001` | `0x00000314` |
   | ConsoleDataBlock | `0xA0000002` | `0x000000CC` |
   | TrackerDataBlock | `0xA0000003` | `0x00000060` |
   | ConsoleFEDataBlock | `0xA0000004` | `0x0000000C` |
   | SpecialFolderDataBlock | `0xA0000005` | `0x00000010` |
   | DarwinDataBlock | `0xA0000006` | `0x00000314` |
   | IconEnvironmentDataBlock | `0xA0000007` | `0x00000314` |
   | ShimDataBlock | `0xA0000008` | `>= 0x00000088` |
   | PropertyStoreDataBlock | `0xA0000009` | `>= 0x0000000C` |
   | KnownFolderDataBlock | `0xA000000B` | `0x0000001C` |
   | VistaAndAboveIDListDataBlock | `0xA000000C` | `>= 0x0000000A` |

   Terminated by a `u32` `< 0x00000004`. Note `0xA000000A` is unassigned.
   `PropertyStoreDataBlock` contains an [MS-PROPSTORE](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-propstore/39ea873f-7af5-44dd-92f9-bc1f293852cc)
   serialized property storage — treat as opaque bytes in v1, parse later if the AppUserModelID
   is ever needed.
5. **A serializer.** The template has none, and writing `.lnk` is what makes "drag a shortcut out
   of an Azul app into Explorer" work. This is arguably more valuable than the reader.
6. Drop the `time 0.1` dependency (yanked-era, unmaintained). Expose `FILETIME` as a raw
   `u64` 100ns-since-1601 plus an optional `time`/`chrono` feature.

**A `.lnk` parser must never resolve or execute anything.** Parse to data, return it. This format
has a long CVE history (CVE-2010-2568 / Stuxnet, CVE-2017-8464) precisely because shells act on
parsed content.

### 4.9 `rclip-file-desc` — virtual / promised files

`CFSTR_FILEDESCRIPTORW` = `FILEGROUPDESCRIPTORW { UINT cItems; FILEDESCRIPTORW fgd[]; }`, each
descriptor 592 bytes: `dwFlags, clsid, sizel, pointl, dwFileAttributes, ftCreationTime,
ftLastAccessTime, ftLastWriteTime, nFileSizeHigh, nFileSizeLow, cFileName[260]` (UTF-16).
`dwFlags` bits say which fields are meaningful (`FD_FILESIZE 0x40`, `FD_WRITESTIME 0x20`,
`FD_ATTRIBUTES 0x04`, `FD_LINKUI 0x8000`, `FD_PROGRESSUI 0x4000`).

This is how Outlook drags an attachment that isn't on disk, and how an Azul app could offer
"drag this generated PDF into Explorer" without writing a temp file. The *bytes* arrive via
`CFSTR_FILECONTENTS` `IStream`s — transport, not here — but the descriptor is a plain struct.

### 4.10 The shortcut family

Four formats, one concept — "a file that points somewhere". Worth doing together so they share
one `ShortcutTarget` type and can convert between each other.

- **`rclip-url-file`** — Windows `.url`. INI-ish: `[InternetShortcut]` with `URL=` (only required
  key), plus `IconFile=`, `IconIndex=`, `HotKey=`, `ShowCommand=`, `Modified=`, `IDList=`.
  Undocumented but stable; also `[InternetShortcut.A]`/`.W` sections for encoded URLs.
- **`rclip-webloc`** — macOS `.webloc` / `.inetloc`. A plist (XML *or* `bplist00` binary — Finder
  drag-created ones are binary) whose single dict has key `URL`. `.inetloc` adds `URLName`.
  Use the `plist` crate; this crate is thin, mostly the both-encodings handling and the
  legacy resource-fork (`url `/`drag`) variant for pre-OSX files.
- **`rclip-desktop-entry`** — freedesktop [Desktop Entry Spec 1.5](https://specifications.freedesktop.org/desktop-entry/latest/).
  The Linux shortcut. INI groups, `[Desktop Entry]`, `Type=Application|Link|Directory`.
  `Type=Link` + `URL=` is the direct `.url`/`.webloc` analogue. Needs: the escape sequences
  (`\s \n \t \r \\`), `;`-separated lists with escaped separators, localized keys
  `Name[de_DE]=`, and — for `Type=Application` — `Exec=` field codes (`%f %F %u %U %i %c %k`)
  with the spec's quoting rules. Several crates exist (`freedesktop_entry_parser`,
  `freedesktop-file-parser`); evaluate before writing. Reuse if the escape handling is correct,
  which is the part they usually get wrong.
- **`rclip-bookmark`** — macOS `BookmarkData`. The real macOS "alias": what `NSURL.bookmarkData`
  returns and what Finder alias files contain. Fully reverse-engineered
  ([mac_alias](https://mac-alias.readthedocs.io/en/latest/bookmark_fmt.html)). Clean structure,
  little-endian throughout except dates:

  ```
  header  48 bytes: magic 'book'|'alis' | u32 total_size | u32 0x10040000 | u32 header_size=48 | 32 reserved
  @48:    u32 offset to first TOC
  TOC:    u32 size(-8) | u32 magic 0xFFFFFFFE | u32 id | u32 next_toc | u32 count | entries[]
  entry:  u32 key (bit31 set => lower bits are a string-record offset) | u32 data_offset | u32 0
  record: u32 length | u32 type | payload
  ```
  Types: `0x0101` UTF-8 string, `0x0201` bytes, `0x0303` i32, `0x0304` i64, `0x0400` date
  (**big-endian** f64 seconds since 2001-01-01), `0x0500/0x0501` false/true, `0x0601` array,
  `0x0701` dict, `0x0801` UUID, `0x0901` URL. Keys: `0x1003` target URL, `0x1004` path
  components, `0x1020` filename, `0x2002` volume path, `0x2010` volume name, `0xF017` display
  name, `0xF080/0xF081` sandbox extensions.

  Value beyond aliases: a bookmark survives the file being *moved*, which a `file://` URL does
  not. Reading them is how you resolve a Finder alias that was dragged in.

---

## 5. Testing

The formats are defined by what real applications actually emit, which frequently differs from
the spec. So the test strategy is corpus-first, not spec-first.

**a. Corpus.** `corpus/<platform>/<source-app>/<flavor>.bin`, checked into git, each with a
`.json` sidecar recording OS version, source app + version, and what was copied. Target sources:
Word, Outlook, Excel, Explorer, Chrome, Firefox, Paint (Windows); Pages, TextEdit, Safari,
Finder, Preview, Mail (macOS); LibreOffice, Nautilus, Dolphin, GIMP, GTK4 + Qt6 demo apps (Linux).

**b. Capture tools.** `tools/dump-clipboard` per platform: enumerate every offered format and
write each to disk. ~200 lines each, and they double as the manual debugging tool you will reach
for constantly.

**c. Round-trip properties.** For every codec: `parse(serialize(x)) == x`. For every corpus file:
`parse(f)` succeeds, and where the format is canonical, `serialize(parse(f)) == f` byte-for-byte.
Where it isn't (RTF, CF_HTML), assert semantic equality of the re-parse.

**d. Oracle tests on the native OS.** This is the part that actually proves interop, and it needs
real applications, not mocks:
- Windows: write our blob with `SetClipboardData`, read back through `IHTMLDocument`/`RichEdit`;
  paste into WordPad and screenshot-diff.
- macOS: `NSAttributedString(rtf:documentAttributes:)` as the oracle for the RTF writer —
  if AppKit round-trips our RTF to the same attributed string, it is correct by definition.
- Linux: `xclip -selection clipboard -t <target> -o` and `wl-paste --list-types` against a running
  GTK4 and Qt6 app; drive paste into LibreOffice via its UNO API.

These run in a `#[cfg]`-gated `e2e` test suite, not in the default `cargo test`.

**e. Fuzzing.** One `cargo-fuzz` target per parser, seeded from the corpus, in CI. Non-negotiable
— see below.

## 6. Security

**Every parser here consumes bytes written by another process.** A hostile or merely buggy
application can put anything on the clipboard, and the receiving app parses it without the user
doing more than pressing Ctrl+V. Treat all input as adversarial:

- `#![forbid(unsafe_code)]` in every codec crate.
- **Never allocate based on a length field** without checking it against remaining input.
  `Vec::with_capacity(header.count)` on a `u32` read from the wire is a one-line OOM.
- All offsets validated against the buffer before use — `CIDA.aoffset`, `LinkInfo` offsets and
  `CF_HTML`'s `StartHTML` are all attacker-controlled indices into the blob.
- Bound recursion: RTF group nesting, PIDL chains, bookmark dict nesting. Depth limit + explicit
  error, no stack overflow.
- Parsers return data, never perform actions. No path resolution, no file access, no execution.
- Size caps on decoded output (a 12-byte DIB header can claim a 4-billion-pixel image).

## 7. Sequencing

**Phase 0 — foundation.** `rclip-core` (flavor enum, registry, `Reader`, errors). Capture tools
for all three platforms; seed the corpus. Change azul's two `get/set_system_clipboard` signatures
to `ClipboardPayload` with only the text flavor wired — no behavior change, but every later phase
becomes additive.

**Phase 1 — the four that pay off immediately.** `cf-html`, `uri-list`, `dropfiles`, `dib`.
Together these deliver: paste formatted text from a browser, paste a screenshot, drag files in,
copy files out — on all four backends. Highest value per line of code in the whole plan.

**Phase 2 — `rtf`.** The big one. Unlocks macOS rich text properly and raises Windows fidelity.
Budget real time for it; use AppKit as the oracle.

**Phase 3 — Windows shell.** `idlist` → finish `shell-link` (reader *and* writer) →
`file-desc`. Unlocks shortcut drops and virtual-file drag-out.

**Phase 4 — the shortcut family.** `url-file`, `webloc`, `bookmark`, `desktop-entry` behind a
shared `ShortcutTarget`. This is the "drag a link into the OS and get structural data back" goal.

**Phase 5 — facade + azul wiring.** `rich-clipboard` umbrella crate, feature-gated per format.
Wire flavor negotiation into the four platform backends. Extend azul's `ClipboardContent` to
carry images and file lists alongside styled runs.

Phases 1–4 are independent of each other once Phase 0 lands, so they can be reordered or run in
parallel by priority.

## 8. Open decisions

1. **Publish to crates.io, or vendor into azul?** Affects naming and the `no_std` question.
   Recommendation: publish — these are generally useful and the corpus/fuzzing effort only pays
   off with outside users filing bugs.
2. **`no_std + alloc`?** All of these can be. Costs a little ergonomics (no `std::io::Read`,
   no `PathBuf`). Recommendation: yes for the codecs, `std` for the facade — it keeps the door
   open for azul's web/wasm target.
3. **One `RichText` hub type, or pairwise conversions?** Recommendation: hub. N formats, one
   intermediate; pairwise is N².
4. **Reuse `freedesktop_entry_parser` / `plist` / `image`, or write our own?** Recommendation:
   reuse `plist` and `image` (mature, correct); audit the desktop-entry crates for escape-sequence
   handling and write our own if they're wrong.

## 9. Spec index

See [`specs.md`](specs.md).

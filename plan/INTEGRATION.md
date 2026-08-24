# Integrating rich-clipboard into azul

Written for whoever wires these crates into azul. It assumes you know azul and nothing about this
workspace.

Read [`PLAN.md`](PLAN.md) for why the workspace is shaped this way. This document is the how.

---

## 1. What you are being handed, and what you still have to write

Clipboard support splits into three layers. **Two of them are done; the first is yours.**

| Layer | What it is | Status |
|---|---|---|
| **1. Transport** | `OpenClipboard`/`IDataObject`, `NSPasteboard`, ICCCM selections + INCR, `wl_data_offer` fds, XDND | **Yours.** Partly exists in azul already |
| **2. Codecs** | `&[u8] → T` and back, for 14 formats | Done and published. `no_std`, no `unsafe`, 854 tests |
| **3. Policy** | Which flavor to prefer, lossy conversions, size limits | Done, in `rich-clipboard` |

Nothing in this workspace calls an OS API. That is deliberate — it is what makes a Windows `.lnk`
parser testable on a Mac — and it means the OS half was never written here and is not hiding
somewhere.

**Your job is to produce and consume one type**: `ClipboardPayload`. Everything else follows.

## 2. The seam

Azul's clipboard is plain text on all four backends. Two functions:

- `dll/src/desktop/shell2/common/event.rs:273` — `fn get_system_clipboard() -> Option<String>`
- `dll/src/desktop/shell2/common/event.rs:298` — `fn set_system_clipboard(text: String) -> bool`

They become:

```rust
fn get_system_clipboard() -> Option<ClipboardPayload>;
fn set_system_clipboard(payload: ClipboardPayload) -> bool;
```

**Do this first, with only the text flavor wired.** It is a no-behavior-change refactor and it makes
every later step additive rather than a rewrite. `ClipboardContent`
(`layout/src/managers/selection.rs:48`) already has the right shape for rich *text* — `plain_text`
plus `styled_runs` — and no representation for images, file lists or links; extend it after the
seam moves, not before.

## 3. The API

```toml
[dependencies]
rich-clipboard = { version = "0.1", features = ["rich-text", "file-list", "dib", "shortcut"] }
```

Everything is on crates.io at `0.1` — sixteen crates. You want `rich-clipboard`; the fifteen
below it (`rclip-core` plus fourteen codecs) are there too, if you ever need one on its own — parsing a
`.lnk` in a build script, say, without the rest of a clipboard stack.

Every format is a separate feature and all are **off by default** — a consumer that wants text and
images must not compile the `.lnk` parser. `full` turns on all formats; `image` (an optional
dependency on the `image` crate, for PNG/TIFF encoding) is deliberately *not* in `full`, because it
is a delegation rather than a format.

| Feature | Gives you |
|---|---|
| `rich-text` | `html` + `rtf`: styled text both directions |
| `file-list` | `CF_HDROP` and `text/uri-list`, including the GNOME/KDE cut conventions |
| `dib` | `CF_DIB` / `CF_DIBV5` images |
| `shortcut` | `.url`, `.webloc`, `.desktop`, macOS bookmarks |
| `shell-link` | `.lnk`, pulls `id-list` |
| `file-desc` | Virtual/promised files (`FILEGROUPDESCRIPTOR`) |
| `image` | PNG/TIFF encode, via the `image` crate. Needed to publish an image on macOS at all |

### Reading

```rust
use rich_clipboard::{decode_payload, RichItem};

match decode_payload(&payload)? {
    RichItem::RichText(rt) => { /* rt.text, rt.runs */ }
    RichItem::Text(s)      => { /* plain */ }
    RichItem::Image(img)   => { /* img.rgba(), width, height */ }
    RichItem::Files(list)  => { /* list.entries() */ }
    RichItem::Shortcut(s)  => { /* s.target() */ }
    RichItem::Link(l)      => { /* l.url, l.title */ }
    other => { /* Html, ShellItems, PromisedFiles, Unknown */ }
}
```

`decode_payload` picks the richest flavor on offer by `Flavor::read_rank`, falling through to the
next when one fails. `decode_all` returns every flavor decoded. `decode` takes a single item.

### Writing

```rust
use rich_clipboard::{encode, Platform};

let payload = encode(&RichItem::RichText(rt), Platform::native())?;
set_system_clipboard(payload);
```

`encode` fans one item out to *every* flavor that platform wants — on Windows, styled text becomes
RTF **and** CF_HTML **and** CF_UNICODETEXT simultaneously. That fan-out is what makes a paste land
in Word as styled text rather than flattened. `write_plan` returns the plan without executing it,
with a `Fidelity` on each entry (`Full` / `Lossy` / `Sidecar`) so you can see what a flavor costs.

## 4. Per-platform transport notes

This is where the work is. Each entry is something that was measured during this project, not read
in a document.

### macOS

- **`-[NSPasteboard dataForType:]` silently loses files.** A pasteboard holds *items*, and the
  pasteboard-level API only reaches the first item offering a type — so a three-file copy reads
  back as **one file**. Walk `pasteboardItems` and record which item each representation came from
  in `ClipboardItem::item`. `ClipboardPayload::all(flavor)` then returns all of them;
  `get(flavor)` is first-match and is the wrong tool for a file list.
- **Finder does not put a path on the pasteboard.** `public.file-url` is the opaque
  `file:///.file/id=<volume>.<inode>` form. Resolving it needs the filesystem, which is why no
  codec does it — that resolution is yours if you need a path.
- **Every modern UTI has a byte-identical legacy twin** (`public.rtf` / `NeXT Rich Text Format v1.0
  pasteboard type`, and six more, verified with `cmp`). The registry resolves both to the same
  `Flavor`, so **dedupe** or you will do everything twice.
- **Promised types that never arrive are normal.** Safari advertises
  `com.apple.linkpresentation.metadata` and `dataForType:` returns nil. Skip, don't fail.
- `public.utf16-external-plain-text` is little-**endian** with an `FF FE` BOM, despite "external"
  conventionally meaning network byte order. It maps to `Flavor::PlainTextUtf16`, not `PlainText`.
- Finder's plain-text form separates names with **CR** (`0x0D`), not LF.

### Windows

- **`CF_UNICODETEXT` is NUL-terminated** and a consumer calling `lstrlenW` on a buffer without one
  reads past the allocation. `rich_clipboard`'s encoder adds it; if you build a payload by hand,
  you must too.
- **`CF_RTF` arrives NUL-terminated off the clipboard**, which the RTF spec never mentions.
- **`CF_DIBV5` has no agreed alpha convention.** Chromium and Firefox write premultiplied; XnView
  and Photoshop read the same bytes as straight; nothing in the format says which. `Options::alpha`
  is the knob and the default is a documented one-directional heuristic, not a detector. If you
  know the source application, say so explicitly.
- Predefined formats have no name from `GetClipboardFormatNameW`. Map the numbers with
  `WindowsFormat::name()`, which is what `Flavor::from_windows_name` reads back.
- `CF_BITMAP` and `CF_ENHMETAFILE` are **handles, not bytes**. Report them; do not pretend to dump
  them.

### X11

- **INCR is mandatory, not optional.** Anything over roughly 256 KB arrives that way, and a
  screenshot always will.
- **INCR is also where your size limit comes from.** ICCCM: the `INCR` property's value "represents
  a lower bound on the number of bytes of data in the selection". That is a `SizeHint::AtLeast`,
  and it arrives *before* the transfer starts. See §5.
- Request `TARGETS` first, then each target in turn.

### Wayland

- Each MIME type is a **pipe fd you read to EOF**, and the protocol never states a length. This is
  the one platform where the size is unknowable in advance — `SizeHint::Unknown`. Count bytes as
  they arrive with `rclip_core::Budget` and stop; there is no other defence.

### Linux, both display servers

These conventions carry cut-versus-copy and **none of them is specified**. Without them, every
paste of files reads as a copy.

- `x-special/gnome-copied-files`: first line is literally `copy` or `cut`, then `file://` URIs.
  **It must carry no trailing newline** — since Nautilus 44 an empty line or a CRLF makes the
  reader reject the *whole payload*, and the paste silently does nothing.
- `application/x-kde-cutselection`: `"1"` means cut. It writes **`"0"` for copy**, so
  payload-present does not mean cut — test the byte.
- `x-special/nautilus-clipboard` was **never a MIME type**. It was a magic first line inside
  `text/plain`, and it is gone since Nautilus 40. `rclip-uri-list` parses it for old producers and
  deliberately never emits it.
- KDE and GNOME do not read each other's; Chromium implements neither and has no cut/copy for
  Linux files at all. `rclip_uri_list::emit::RECOMMENDED` says which to publish (answer: all of
  them).

### Every platform

- **`text/html`'s encoding is unreliable, and the naive fix does not work.** `"<b>hi</b>"` in
  UTF-16LE is `3C 00 62 00 …` — and every one of those bytes is a legal UTF-8 character, NUL
  included. So a reader that tries UTF-8 first and falls back on failure **never falls back**, and
  hands you a string with a NUL between every letter. `rclip_core::text::decode_html_bytes` does
  BOM, then the interleaved-NUL check, then UTF-8, in that order, and that order is the point.

## 5. Size, and the callback

**Measured behaviour** on 256 MiB of hostile input: no parser panics and none OOMs. Field-level
guards reject impossible fields, and flat list iterators are linear in their input — the worst
density in the workspace is `rclip-idlist` at 0.5 items per byte — so capping the input
transitively caps the item count.

What that does *not* cover is graph-shaped formats, where objects are addressed by index and one
container can name the same object many times. Both such formats here carry a node budget for
exactly that reason: `rclip-bookmark` and `rclip-webloc`'s bplist. A 223-byte bplist that fans out
nine wide over nine levels resolves 40 million objects — while never exceeding depth 9, so a depth
limit never fires. **Depth alone is not enough** for that shape; if you add a graph format, budget it.

### Two lines of defence, and yours is the first

```rust
pub enum SizeHint { Exact(u64), AtLeast(u64), Unknown }
```

| Platform | What you can know before reading | How |
|---|---|---|
| Windows | `Exact` | `GlobalSize` on the `GetClipboardData` handle; `IStream::Stat` for `TYMED_ISTREAM` |
| macOS | `Exact` | `-[NSData length]` before copying into a `Vec` |
| X11 | `AtLeast` | the `INCR` property's lower bound |
| Wayland | `Unknown` | nothing; use `Budget` while reading |

On Windows and macOS the data is already resident and owned by the clipboard server, so asking its
size costs nothing and no copy has happened yet. **Ask before you copy.**

The asymmetry is the design constraint: a lower bound can prove a payload *too big* but never that
it is small enough, and `Unknown` proves neither. `SizeHint::Unknown.known_bytes()` returns `None`
rather than `0` — treating no-information as small is exactly what makes an unbounded pipe read the
way in.

The facade's limits are the **second** line. By the time a `ClipboardPayload` exists the bytes are
resident, so `Options::limits` cannot stop a huge payload arriving; it stops you *decoding* one,
which is where the amplification is (a 60 MB 8-bit `CF_DIB` becomes 240 MB of RGBA).

```rust
let item = decode_payload_policy(
    &payload,
    &Options::new().limits(my_limits),
    &mut |flavor: Flavor<'_>, hint: SizeHint, _: &Limits| {
        // Surface this to the app: "that paste is 400 MB, still want it?"
        Oversize::Skip
    },
)?;
```

`Skip` is the default and usually right — it falls through to the next-best flavor, so the 400 MB
TIFF goes and the plain text stays. `Accept` decodes anyway, `Abort` fails the paste.

Closure parameters need annotating (`Flavor<'_>`, `SizeHint`, `&Limits`) because `Flavor` borrows
and inference will not produce the higher-ranked bound on its own.

## 6. Verifying your work

- **`cargo run -p dump-clipboard -- --list`** prints every flavor currently on the clipboard, with
  its resolved `Flavor` and byte length. This is the tool for "what did that application *actually*
  put there". The macOS backend is verified against a real pasteboard; **the Windows, X11 and
  Wayland backends were written from the API docs and have never been run** — expect to fix them,
  and that is a good first task because it exercises the same APIs the real transport needs.
- **`corpus/macos/`** holds 27 real captures from TextEdit, Safari, Finder and Preview. `corpus/`
  is empty for the other three platforms; filling it is the highest-value thing you can contribute
  back, and `dump-clipboard` writes the sidecars for you. Read
  [`corpus/README.md`](../corpus/README.md) first — it is a public repo and the redaction rules are
  binding. A leak scanner in CI will fail the build if you miss something.
- **`cargo test --workspace --all-features`** — 854 tests.
- **`fuzz/`** — 31 targets. `fuzz/run-all.sh 120` runs the lot.

## 6b. A note on trusting this document

Everything in §4 was measured on a real machine against real applications, and where a
specification and reality disagreed the code follows reality and says so in a comment. But only
**macOS** was measured directly. The Windows, X11 and Wayland notes come from the specifications
plus the source of real implementations (Wine, GLib, KIO, Nautilus, Chromium), and they have not
been checked against a running system.

Treat the macOS section as observed and the other three as well-researched but unverified. If one
of them turns out to be wrong, that is a finding worth writing down rather than a surprise.

## 7. What is not done

Honest list.

- **Transport, all four platforms.** The point of this document.
- **`SizeHint` production.** The types exist and nothing produces them yet, because only the
  transport can.
- **`CFSTR_FILECONTENTS`** — virtual file *contents* arrive as an `IStream`. The descriptor is
  parsed (`rclip-file-desc`); the stream is transport and is yours.
- **XDND** and the Wayland drag protocols. `text/uri-list` and the drop-effect conventions are
  parsed; the message flow is not implemented.
- **`com.apple.webarchive`** is recognised as a `Flavor` but has no codec. It is the richest thing
  Safari offers.
- **A code-page-aware ANSI path** is behind a default-off `codepage` feature on five crates. Turn it
  on if you care about pre-Unicode Windows payloads.
- **`rclip-html` has no CSS cascade** — a fragment styled by class arrives unstyled. Small in
  practice, because browsers inline computed styles when they serialize a clipboard fragment.

## 8. If something looks wrong

Every non-obvious decision in this workspace has its reasoning in a doc comment next to the code,
and the surprising ones have a test whose name is the claim. Several specs contradict themselves or
contradict reality, and where that happens the code follows what was measured and says so — see the
"Where the spec and observed reality disagree" section in each crate's README.

If a codec rejects something a real application produced, that is a corpus gap and a bug, in that
order. Capture it with `dump-clipboard`, add it to `corpus/`, and the sidecar will tell the next
person what it proved.

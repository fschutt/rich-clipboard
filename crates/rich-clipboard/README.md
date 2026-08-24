# rich-clipboard

The one crate a consumer depends on. It turns a `ClipboardPayload` — a set of platform-native
identifiers and undecoded bytes — into typed data, and typed data back into the *set* of flavors
that thing should be published as.

```
transport            codecs                    policy
IDataObject          rclip-cf-html             rich-clipboard
NSPasteboard   ──▶   rclip-rtf          ──▶    decode: which flavor wins
ICCCM / Wayland      rclip-dib …               encode: which flavors to publish
(in azul)            (twelve crates)           RichText: the conversion hub
```

Layer 1 is not here and never will be — no OS call, no syscall, no `unsafe`. Layer 2 is the
twelve `rclip-*` crates, each one a byte format with no idea what an operating system is. This
crate is layer 3, and layer 3 is two tables and the conversions between them.

## Reading

```rust
use rclip_core::{ClipboardPayload, Platform};
use rich_clipboard::{decode_payload, RichItem};

let payload = ClipboardPayload::new(Platform::MacOs)
    .with("public.utf8-plain-text", &b"hello"[..])
    .with("public.rtf", &br"{\rtf1\ansi\b hello\b0}"[..]);

// RTF outranks plain text, so the styling survives.
match decode_payload(&payload)? {
    RichItem::RichText(text) => assert!(text.runs[0].style.bold),
    _ => unreachable!(),
}
```

`decode_payload` walks the offered flavors in `Flavor::read_rank` order, skips the ones this
build was not compiled to understand, and folds in the metadata flavors a per-item decode cannot
see: a `Preferred DropEffect` becomes `FileList::action`, a `public.url-name` becomes
`Link::title`, a plain-text sibling becomes `HtmlFragment::plain`.

## Writing

This is the half that matters, and it is not the read side reversed.

```rust
let payload = encode(&RichItem::RichText(text), Platform::Windows)?;
// -> "Rich Text Format", "HTML Format", "CF_UNICODETEXT"
```

The receiving application decides what it takes, and it is not going to tell you first. So the
answer is every flavor the item can be expressed in, published at once, best first. Offer only
plain text and the paste into Word is flat; offer only HTML and the paste into WordPad is
nothing.

### The fan-out table

`fanout::write_plan(kind, platform)` is public, so a transport can see what it is about to
publish and what each flavor costs before it commits.

| Item | Windows | macOS | X11 / Wayland |
|---|---|---|---|
| **Text** | `CF_UNICODETEXT` | `public.utf8-plain-text` | `text/plain;charset=utf-8` |
| **RichText** | `Rich Text Format`, `HTML Format`, `CF_UNICODETEXT`ᴸ | `public.rtf`, `public.utf8-plain-text`ᴸ | `text/html`, `text/plain;charset=utf-8`ᴸ |
| **Html** | `HTML Format`, `CF_UNICODETEXT`ᴸ | `public.html`, `public.utf8-plain-text`ᴸ | `text/html`, `text/plain;charset=utf-8`ᴸ |
| **Image** | `CF_DIBV5`, `PNG`, `CF_DIB`ᴸ | `public.png`, `public.tiff` | `image/png` |
| **Files** | `CF_HDROP`, `Preferred DropEffect`ˢ | `public.file-url` × N | `text/uri-list`, `x-special/gnome-copied-files`, `x-special/mate-copied-files`, `application/x-kde-cutselection`ˢ |
| **PromisedFiles** | `FileGroupDescriptorW` | — | — |
| **Link** | `UniformResourceLocatorW`, `CF_UNICODETEXT`ᴸ | `public.url`, `public.url-name`ˢ, `public.utf8-plain-text`ᴸ | `text/uri-list`, `text/plain;charset=utf-8`ᴸ |
| **Shortcut** (`.lnk`) | — | — | — |
| **ShellItems** | — | — | — |

ᴸ lossy, ˢ sidecar (metadata about the flavor before it). The first entry of every plan is
always full-fidelity; there is a test for that.

The decisions worth arguing about, and why they went the way they did:

- **RTF before HTML on Windows.** `PLAN.md` §4.3 calls RTF the higher-fidelity of the two there,
  and `Flavor::read_rank` agrees — so the write side agrees too, rather than having the two
  tables contradict each other on a question they both have an opinion about. Order on Windows
  is a stated preference more than a mechanism: nearly every rich-text consumer asks for a
  specific format id rather than taking the first one `EnumClipboardFormats` yields, so what
  actually decides the paste is the *set*.
- **No `public.html` on macOS.** `public.rtf` is *the* rich flavor there — Pages, TextEdit, Mail
  and Notes all speak it and several speak no HTML at all. Adding HTML wants the AppKit oracle
  test from `PLAN.md` §5d first; the failure mode of guessing wrong is TextEdit pasting raw
  markup. `// TODO(phase-5)`.
- **`CF_DIBV5` before `PNG` on Windows, where reading prefers the opposite.** Reading prefers
  PNG because its alpha convention is unambiguous and `CF_DIBV5`'s famously is not. Writing
  leads with `CF_DIBV5` because Paint, older Office and a long tail of Win32 applications read
  DIB and nothing else, so a PNG-only offer pastes as nothing in Paint. Two different questions;
  the reason the write side needs its own table.
- **No `image/bmp` on X11 or Wayland.** A BMP on that clipboard is a *file* — `BM` magic,
  14-byte `BITMAPFILEHEADER` — and `CF_DIB` is exactly those bytes with the header removed.
  `rclip-dib` writes only the packed form, so offering `image/bmp` would advertise something
  gdk-pixbuf and Qt cannot open.
- **All three Linux file conventions.** They do not read each other's: GNOME ignores KDE's flag,
  KDE ignores GNOME's verb line, and a receiver that knows neither still gets the files out of
  `text/uri-list`. Publish one and two thirds of the desktop reads the user's cut as a copy.

## `RichText`, the conversion hub

`PLAN.md` §4.3 and open decision 3 both ask for one intermediate representation rather than
pairwise conversions, and the arithmetic is the argument: four representations of styled text is
twelve pairwise conversions and eight through a hub, and the next format costs two rather than
eight.

**It can represent** one flat string with character-level formatting over it: bold, italic,
underline, strikethrough, point size, font family, foreground and background colour. Paragraph
breaks are `\n`.

**It cannot represent** anything structural — paragraph alignment, indents and spacing, lists,
tables, inline images, hyperlinks, superscript and subscript, underline *style* (dotted, wavy,
double all collapse to a boolean), or anything a format models as a field or an object. That is
a real ceiling, not a phase-1 shortcut: a hub that could represent everything would be a
document model, and every conversion into it from a format that has less would have to invent
the difference.

**The missing leg is HTML → `RichText`.** It converts *to* HTML and *both ways* with RTF.
Turning markup into runs needs an HTML tokenizer; `rclip-cf-html` states outright that it does
not parse HTML, and nothing else in the workspace does either. So decoding an HTML flavor yields
`RichItem::Html` — the markup, intact — rather than a tag-stripped approximation. It is less of
a hole than it looks: it is exactly why `read_rank` puts RTF above HTML, so when a source offers
both (Word, Outlook, LibreOffice all do) the read side takes the one that becomes structure.

## Features

Every format is behind a feature and **every format is off by default**. An application that
pastes text and images must not compile a `.lnk` parser to do it. `std` is on by default, and
the `no_std + alloc` path is kept working because azul targets wasm (`PLAN.md` §8.2).

| Feature | Pulls | Covers |
|---|---|---|
| `html` | `rclip-cf-html` | `CF_HTML`, `public.html`, `text/html` |
| `rtf` | `rclip-rtf` | `Rich Text Format`, `public.rtf`, `text/rtf` |
| `dib` | `rclip-dib` | `CF_DIB`, `CF_DIBV5` |
| `file-list` | `rclip-dropfiles`, `rclip-uri-list` | `CF_HDROP`, `text/uri-list`, GNOME/KDE conventions |
| `id-list` | `rclip-idlist` | `CFSTR_SHELLIDLIST` |
| `shell-link` | `rclip-shell-link` (+ `id-list`) | `.lnk` |
| `file-desc` | `rclip-file-desc` | `CFSTR_FILEDESCRIPTORW` |
| `url-file` | `rclip-url-file` | `.url` |
| `webloc` | `rclip-webloc` | `.webloc` / `.inetloc` |
| `bookmark` | `rclip-bookmark` | macOS `BookmarkData` |
| `desktop-entry` | `rclip-desktop-entry` | `.desktop` |
| `rich-text` | — | `html` + `rtf` |
| `shortcut` | — | `url-file` + `webloc` + `bookmark` + `desktop-entry` |
| `full` | — | everything |

`file-list` is one feature for two crates on purpose: "a list of files" is one flavor with two
encodings, and a consumer that wants it wants it on every platform it ships to.

A flavor whose feature is off is not silently skipped. `decode` returns `Error::FeatureDisabled`
naming the Cargo feature to turn on; `decode_payload` moves to the next-best flavor and reports
it only if nothing else worked; `encode` drops that flavor and publishes the rest, because a
paste that is worse beats a paste that fails.

With no format features at all the crate still decodes and publishes plain text, and still
carries unknown flavors through verbatim — which is what makes a clipboard bridge possible.
With `--no-default-features` (no `alloc`) what remains is the fan-out table, which is data.

## What lives here that should live somewhere else

Each of these is a codec-side gap the facade works around rather than going without. They are
listed so the workaround is visible instead of quietly permanent.

- **An RTF writer.** `rclip-rtf` has none — its `lib.rs` carries `// TODO(phase-2): the writer`.
  The facade needs one, because on macOS `public.rtf` is the only rich flavor most applications
  read, so a fan-out that cannot produce RTF cannot publish styled text there at all. The
  writer in `rich_text::rtf_write` is written to the rules that crate's README states for the
  writer it is going to grow — minimal font and colour tables, `\uc1`, every non-ASCII character
  as `\uN?`, never a raw high byte — so the swap is a deletion. `// TODO(phase-2)`.
- **A percent-*encoder*.** `rclip-uri-list` decodes and validates percent-encoding but does not
  produce it; its `emit` module writes URIs through verbatim and says "percent-encoding them is
  the caller's job". `encode::to_uri` is the mirror of `Uri::percent_decode` and belongs next to
  it. `// TODO(phase-5)`.
- **The `text/html` encoding sniff.** `PLAN.md` §4.1 lists "`text/html` encoding is unreliable"
  as one of two traps to encode in the registry rather than in each caller. It is not in
  `rclip-core`, so it is in `text::decode_html_bytes` here. Note the ordering it exists for:
  `"<b>"` in UTF-16LE is `3C 00 62 00 3E 00`, which is **valid UTF-8**, so a reader that tries
  UTF-8 first and falls back on failure never falls back at all. `// TODO(phase-5)`.
- **An owned `ShortcutTarget`.** Four codec crates carry a byte-identical borrowed copy with a
  `// TODO(phase-4): hoist this into rclip-core` against each. `shortcut::LinkTarget` is the
  owned consumer-facing version; when the hoist happens this becomes a conversion rather than a
  redefinition.

## What is advertised and not yet filled

- **`CF_DIB` on Windows.** `rclip-dib`'s encoder emits exactly one shape, 32-bpp `BI_BITFIELDS`
  `BITMAPV5HEADER`, "because a producer has no reason to write a format that cannot carry
  alpha". The plan lists the `CF_DIB` line and this build never fills it, which costs the Win32
  applications that read `CF_DIB` and not `CF_DIBV5`. `// TODO(phase-5)`.
- **Pixels on macOS.** The macOS image plan is `public.png` and `public.tiff`, and `PLAN.md`
  §4.4 keeps both encoders out of this workspace on purpose. So `Image::Rgba` cannot be
  published on macOS at all — `encode` returns `Error::NothingEncodable`, and there is a test
  that asserts it. A consumer holding `image` should encode first and hand over
  `Image::Encoded`. An optional `image` feature on *this* crate would close it. `// TODO(phase-5)`.
- **Item grouping in `ClipboardPayload`.** A macOS pasteboard models a multi-file selection as N
  *items*, each carrying one `public.file-url`; `ClipboardPayload` is a flat list with no
  grouping, so they travel as repeated entries sharing an identifier. `encode` emits all of
  them and `decode_payload` collects them back, but a per-item `decode` sees one file. Fixing it
  properly means a change to `rclip-core`. `// TODO(phase-5)`.
- **Publishing a `.lnk`.** `Flavor::ShellLink` has no identifier on any platform, because
  Windows has no registered clipboard format for a shell link — a `.lnk` reaches an application
  as a file. `Shortcut::from_lnk` and `Shortcut::to_lnk` are exposed directly; publishing one
  means a promised file whose `CFSTR_FILECONTENTS` are those bytes, and that last step is
  transport.
- **`text/rtf` on X11 and Wayland.** LibreOffice and AbiWord read it, but no capture in the
  corpus shows a Linux application *offering* it, and this table should follow observed
  behaviour rather than lead it. `// TODO(phase-5)`.
- **A `.lnk` target IDList cannot be rebuilt.** `Shortcut::display_path` is a *label* built from
  an `ITEMIDLIST` and does not round-trip, so `to_lnk` writes only the `LinkInfo` path, the
  strings and the environment block. That is deliberate: the shell binds a PIDL by handing the
  bytes back to the namespace extension that owns them, and a reconstructed one resolves to
  something unintended or not at all.

## Security

Everything the codecs promise, this crate does not undo. No path is resolved, no `.desktop`
`Exec=` is expanded, no URL is opened, no PIDL is bound, no file is touched. A
`LinkTarget::Path` is a string that *looks* like a path. A `Shortcut`'s arguments and icon
location are attacker-chosen text and an attacker-chosen path — CVE-2010-2568 and CVE-2017-8464
were both bugs in the *acting*, not in the parsing.

Two places the crate refuses to guess rather than being helpful:

- **A `CF_HDROP` ANSI path** is bytes in the source machine's code page, which is not in the
  payload. It is dropped, not guessed, because a mangled path is worse than a missing one — a
  mangled one gets opened.
- **`TransferAction` defaults to `Copy`.** Guessing "move" deletes a user's files.

## Verify

```sh
cargo test  -p rich-clipboard --all-features      # 76
cargo test  -p rich-clipboard                     # 33, proves the gating works
cargo build -p rich-clipboard --no-default-features
cargo build -p rich-clipboard --no-default-features --features alloc,full
cargo clippy -p rich-clipboard --all-targets --all-features -- -D warnings
cargo fmt -p rich-clipboard -- --check
```

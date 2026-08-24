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

It also reassembles a multi-*item* selection, which is the other thing one item cannot see. A
three-file copy in Finder is three `NSPasteboardItem`s that each offer `public.file-url`, not one
item offering it three times — and `-[NSPasteboard dataForType:]` reaches only the first, which
is how a three-file paste silently becomes a one-file paste. `ClipboardPayload` records the item
index, so `decode_payload` collects across items with `all()` rather than taking the first match
with `get()`, and `encode` emits the same grouping on the way out.

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
| **RichText** | `Rich Text Format`, `HTML Format`, `CF_UNICODETEXT`ᴸ | `public.rtf`, `public.html`, `public.utf8-plain-text`ᴸ | `text/html`, `text/rtf`, `text/plain;charset=utf-8`ᴸ |
| **Html** | `HTML Format`, `CF_UNICODETEXT`ᴸ | `public.html`, `public.utf8-plain-text`ᴸ | `text/html`, `text/plain;charset=utf-8`ᴸ |
| **Image** | `CF_DIBV5`, `PNG`, `CF_DIB`ᴸ | `public.png`, `public.tiff` | `image/png` |
| **Files** | `CF_HDROP`, `Preferred DropEffect`ˢ | `public.file-url`, one pasteboard *item* per file | `text/uri-list`, `x-special/gnome-copied-files`, `x-special/mate-copied-files`, `application/x-kde-cutselection`ˢ |
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
- **`public.rtf` *and* `public.html` on macOS.** RTF first, because it is *the* rich flavor
  there. HTML second, settled by the AppKit oracle `PLAN.md` §5d asks for and then by pasting
  this crate's own output into TextEdit, Pages, Safari, Chrome and Mail on macOS 15.5. Adding
  HTML cannot displace RTF for an AppKit consumer, because the *reader* picks:
  `-[NSTextView readablePasteboardTypes]` is ordered RTFD, RTF, HTML and
  `availableTypeFromArray:` takes the first of *that* list on offer — TextEdit and Pages both
  took the RTF, styling identical either way. The feared failure mode is not real either: offered
  `public.html` alone, an `NSTextView` renders it as styled text, not as literal markup. What the
  HTML offer buys is not a fallback — WebKit and Chromium get something rich either way, because
  macOS converts the RTF for them — but *fidelity*: through Cocoa's RTF-to-HTML writer, Safari
  and Chrome render an 18pt run as `font-size: 18px`, a third smaller than asked for, and force
  Helvetica onto every run. Offered `public.html` they render the fragment verbatim at 18pt.
- **`text/rtf` behind `text/html` on X11 and Wayland, and never `text/richtext`.** The one row
  where the Unix order is not the Windows and macOS order, and the toolkits are the reason: Qt has
  no RTF anywhere — `QTextEdit`'s rich text *is* an HTML subset — and GTK's rich-text clipboard
  target is `application/x-gtk-text-buffer-rich-text`, which GTK's own documentation says "does
  not comply to any standard rich text format and only works between GtkTextBuffer instances". So
  HTML is what the desktop takes. RTF is behind it for the applications underneath the toolkits:
  LibreOffice registers `text/rtf` for `SotClipboardFormatId::RTF`, and AbiWord imports RTF.
  `text/richtext` is *not* offered even though LibreOffice advertises RTF under exactly that name
  on X11 — that type is RFC 1896 enriched text, and emitting RTF under it is mislabelling.
  `Flavor::from_mime` accepts the spelling on the way in, which is the right asymmetry: liberal
  in what the read side takes, conservative in what the write side claims.
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

**Both legs are now attached.** It converts both ways with RTF (through `rclip-rtf`) and both
ways with HTML (through `rclip-html`), so an HTML flavor decodes to `RichItem::RichText` rather
than to markup. Until phase 2 it did not: turning markup into runs needs an HTML tokenizer,
`rclip-cf-html` states outright that it does not parse HTML, and nothing else in the workspace
did either.

What HTML still loses on the way in is the cascade — a fragment that styles its text through a
class rather than through a `style=` attribute arrives unstyled — plus links, images, and lists
and tables as structure. That is why `read_rank` still puts RTF above HTML: when a source offers
both (Word, Outlook, LibreOffice all do) the read side takes the one that needs no stylesheet.

A caller that wants the markup itself can still have it: `Options::keep_html_markup(true)` turns
the decode back into a `RichItem::Html`, which is what a clipboard bridge or an inspector wants
and what carries `SourceURL` and the surrounding context document. That form now also gets a
`plain` rendering filled in from the markup, because there is a tokenizer to produce one.

## Features

Every format is behind a feature and **every format is off by default**. An application that
pastes text and images must not compile a `.lnk` parser to do it. `std` is on by default, and
the `no_std + alloc` path is kept working because azul targets wasm (`PLAN.md` §8.2).

| Feature | Pulls | Covers |
|---|---|---|
| `html` | `rclip-cf-html`, `rclip-html` | `CF_HTML`, `public.html`, `text/html` |
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
| `image` | `image` (optional, off) | encoding `Image::Rgba` as PNG and TIFF |
| `rich-text` | — | `html` + `rtf` |
| `shortcut` | — | `url-file` + `webloc` + `bookmark` + `desktop-entry` |
| `full` | — | everything |

`file-list` is one feature for two crates on purpose: "a list of files" is one flavor with two
encodings, and a consumer that wants it wants it on every platform it ships to.

`image` is the odd one out and is deliberately **not** in `full`: it is not a format this
workspace owns, it is a *delegation*. `PLAN.md` §4.4 scopes PNG and TIFF out of the workspace and
says to delegate, and §8.4 recommends reusing `image` rather than writing one. Turning it on is
the difference between `Image::Rgba` having a macOS representation and having none. It is off by
default because it is by far the largest dependency the crate can pull — a consumer that already
decodes images has an encoder of its own and should hand over an `Image::Encoded`. The
default-feature dependency graph is `rclip-core` and nothing else, exactly as before.

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

- **The `text/html` encoding sniff.** `PLAN.md` §4.1 lists "`text/html` encoding is unreliable"
  as one of two traps to encode in the registry rather than in each caller. It is not in
  `rclip-core`, so it is in `text::decode_html_bytes` here. Note the ordering it exists for:
  `"<b>"` in UTF-16LE is `3C 00 62 00 3E 00`, which is **valid UTF-8**, so a reader that tries
  UTF-8 first and falls back on failure never falls back at all. `// TODO(phase-5)`.
- **An owned `ShortcutTarget`.** The shared borrowed type now lives in `rclip-core` and every
  codec crate in the family re-exports it. `shortcut::LinkTarget` stays as the owned
  consumer-facing version, because a `Link` outlives the blob it was parsed out of, but it is a
  *conversion* of the shared type and no longer a second definition of it: `LinkTarget::classify`
  delegates to `ShortcutTarget::classify`, and `LinkTarget::as_target` and `From<ShortcutTarget>`
  move between the two.

## What is advertised and not yet filled

- **Pixels on macOS, without the `image` feature.** The macOS image plan is `public.png` and
  `public.tiff`, and `PLAN.md` §4.4 keeps both encoders out of this *workspace* on purpose. With
  `image` off there is nothing to call and `encode` returns `Error::NothingEncodable`; with it on
  `Image::Rgba` publishes as both. That is the intended shape — the delegation the plan asks for,
  made optional — rather than a gap, but a build that has not opted in still has one.
- **`;` in a `file://` URI.** The percent-encoder now lives where it belongs — `to_uri`
  delegates to `rclip_uri_list::emit::file_uri`, whose set is RFC 3986 `pchar` plus `/`. Checked
  against real GLib 2.88 over every printable ASCII byte and a range of multi-byte UTF-8, it
  agrees with `g_filename_to_uri` on all of them but one: GLib escapes `;` as `%3B` and this does
  not, because `g_filename_to_uri` does not actually use
  `G_URI_RESERVED_CHARS_ALLOWED_IN_PATH` — it escapes against an *unsafe* list whose allowed
  reserved set is `:@&=+$,` plus `/`. Both spellings decode to the same path, so nothing is lost
  or misread; what differs is a textual comparison against a URI GTK minted itself. There is a
  test pinning it. Fixing it means a change to `rclip-uri-list`.
- **Publishing a `.lnk`.** `Flavor::ShellLink` has no identifier on any platform, because
  Windows has no registered clipboard format for a shell link — a `.lnk` reaches an application
  as a file. `Shortcut::from_lnk` and `Shortcut::to_lnk` are exposed directly; publishing one
  means a promised file whose `CFSTR_FILECONTENTS` are those bytes, and that last step is
  transport.
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
cargo test  -p rich-clipboard --all-features      # 90
cargo test  -p rich-clipboard                     # 34, proves the gating works
cargo build -p rich-clipboard --no-default-features
cargo build -p rich-clipboard --no-default-features --features alloc,full
cargo clippy -p rich-clipboard --all-targets --all-features -- -D warnings
cargo fmt -p rich-clipboard -- --check
```

The two macOS format decisions above were settled against real applications rather than against
this crate's own idea of them: an AppKit harness dumping `-[NSTextView readablePasteboardTypes]`
and running `readSelectionFromPasteboard:`, and then `encode`'s actual output pasted into
TextEdit, Pages, Safari, Chrome and Mail on macOS 15.5. Those are not in `cargo test` — they need
a window server and five applications — and they belong with the `e2e` suite `PLAN.md` §5d
describes.

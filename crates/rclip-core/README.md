# rclip-core

Shared vocabulary for the [`rich-clipboard`](https://github.com/fschutt/rich-clipboard) codecs:
what a clipboard item *is*, a bounds-checked reader for parsing one, and the size policy for
deciding whether to.

Every `rclip-*` codec depends on this crate and on nothing else.

```toml
rclip-core = "0.1"
```

`#![no_std]`, `#![forbid(unsafe_code)]`, no dependencies. `alloc` and `std` are opt-in features.

## What is in it

**`Flavor`** — the cross-platform registry. A clipboard item is "HTML" or "a list of files"
regardless of whether the platform calls it `public.html`, `"HTML Format"` or `text/html`, and this
is the mapping in both directions, plus a preference order for when a source offers several at once.
It knows the things that are easy to get wrong: that every modern macOS UTI has a byte-identical
legacy twin, that `CorePasteboardFlavorType 0xNNNNNNNN` is a four-character OSType in hex, and that
`public.utf16-external-plain-text` is a different flavor from `public.utf8-plain-text` rather than a
spelling of it.

**`Reader`** — a cursor that cannot read out of bounds. Clipboard payloads are written by other
processes and parsed the instant a user presses Ctrl+V, so a raw `buf[off..off + len]` on a length
field read off the wire is a panic waiting to happen. `check_count` is the guard against sizing an
allocation from a `u32` that came off the wire; `take_reader` hands an inner parser a sub-reader
that physically cannot see past its own record.

**`ClipboardPayload`** — everything a source offered, still encoded. A clipboard read is never one
blob: copy a table out of a browser and you get HTML, RTF, an image and plain text simultaneously.
Decoding flavors nobody asked for is wasted work on payloads that are routinely megabytes, so this
holds them undecoded and lets the consumer choose. It also records which pasteboard *item* each
representation belongs to, because a three-file copy on macOS is three items rather than one item
mentioning three files.

**`Limits`, `SizeHint`, `Budget`** — the size policy. `SizeHint` is three-valued because the
platforms genuinely differ: Windows and macOS can give an exact byte count before any copy happens,
X11's `INCR` gives only a lower bound, and Wayland gives nothing at all. A lower bound can prove a
payload too big but never that it is small enough, so `Unknown` returns `None` rather than `0` —
treating no-information as small is what makes an unbounded pipe read the way in.

**`Error`** — one error type, always carrying the byte offset it failed at. That offset is what
makes a fuzz crash reproducible and a corpus mismatch debuggable.

## What is not in it

Any OS call. Transport — `IDataObject`, `NSPasteboard`, ICCCM selections, `wl_data_offer` — belongs
to whatever is driving the clipboard. Keeping this crate free of it is what makes a Windows `.lnk`
parser testable on a Mac.

Any codec. Those are the sibling `rclip-*` crates, and
[`rich-clipboard`](https://crates.io/crates/rich-clipboard) is the umbrella that ties them together.
Most consumers want that rather than this.

## License

MIT OR Apache-2.0

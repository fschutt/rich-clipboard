# rclip-dropfiles

`CF_HDROP` — the Win32 clipboard and drag-and-drop format for "here is a list of existing
files". A 20-byte `DROPFILES` header (`pFiles`, `pt`, `fNC`, `fWide`) followed, at the byte
offset `pFiles` names, by a double-NUL-terminated array of paths. Parser *and* serializer:
this is the format files travel on in both directions, so being able to produce it matters as
much as being able to read it.

Spec: [DROPFILES][ns] and the CF_HDROP section of [Shell Clipboard Formats][shell].

[ns]: https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-dropfiles
[shell]: https://learn.microsoft.com/en-us/windows/win32/shell/clipboard

```rust
use rclip_dropfiles::{Builder, DropFiles, Point};

// Read
let drop = DropFiles::parse(bytes)?;
for path in drop.paths() {            // borrows `bytes`, allocates nothing
    println!("{:?}", path.to_string_lossy());
}

// Write
let mut b = Builder::wide().at(Point::new(120, 80));
b.push_str(r"C:\Users\me\report.pdf")?;
let payload = b.finish();
```

`no_std`, `forbid(unsafe_code)`, no dependency but `rclip-core`. Parsing borrows; the
serializer is behind the `alloc` feature.

## The parts that are easy to get wrong

- **`pFiles` is an offset from the start of the struct, not a constant.** It is 20 in practice,
  and the header is 20 bytes, so hardcoding 20 works right up until it doesn't. This crate
  honours whatever the field says, rejects anything below 20 (which would alias the header) or
  past the end of the buffer, and there is an `offset-padded.bin` fixture with a four-byte gap
  to keep it honest.
- **The array is terminated by an *empty string*, not by "two NULs somewhere".** `a\0b\0\0` is
  two paths; `\0` on its own is zero paths; `a\0\0` is one path. `parse` walks the array once to
  locate the terminator so the path iterator can be infallible, and refuses input where the
  terminator never arrives. `DragQueryFile` would keep reading past the end of the `HGLOBAL`
  there; returning `UnexpectedEof` is the only safe answer.
- **`fNC` and `fWide` are Win32 `BOOL`, i.e. `int`.** Any nonzero value is TRUE. Sources that
  write `-1` exist.
- **`pt` and `fNC` are load-bearing, not decoration.** They carry where a drag was dropped, and
  `fNC` decides whether that point is in screen or client coordinates.
- **Trailing bytes are normal.** The payload arrives in an `HGLOBAL` and `GlobalAlloc` rounds
  capacity up, so zero-padding after the terminator is ignored rather than read as extra empty
  paths.

## Prior art

Nothing on crates.io parses this structure. Searching `dropfiles` returns no results at all;
`hdrop` returns unrelated crates. What exists is FFI, which is exactly why this crate does.

- **[`clipboard-win`](https://crates.io/crates/clipboard-win)** (5.4.1, 54M downloads) — the
  obvious candidate, and not usable. Its `formats::FileList` getter calls `DragQueryFileW`
  rather than parsing the struct, so there is no reusable codec inside it; Windows-only target,
  `std`, and it hands you the clipboard rather than a byte slice. Rejected: we need to parse
  bytes that arrived over RDP, out of a corpus file, or from another OS entirely.
- **[`windows`](https://crates.io/crates/windows) / `windows-sys` / `winapi`** — supply
  `DROPFILES` as a `#[repr(C)]` FFI declaration and nothing else. Using one means an `unsafe`
  pointer cast over an attacker-controlled buffer with no bounds check on `pFiles`, which
  `#![forbid(unsafe_code)]` rules out on purpose. Rejected: a struct declaration is not a
  parser.
- **[`clipboard-files`](https://crates.io/crates/clipboard-files)** (0.1.2) — a thin
  cross-platform "read file paths from the clipboard" wrapper over `clipboard-win`, `gtk` and
  `objc`. Rejected: inherits the FFI problem plus three GUI toolkits.
- **[`ironrdp-cliprdr`](https://crates.io/crates/ironrdp-cliprdr)** (0.7.0) — parses the RDP
  clipboard channel and does have real decoders, but not for `CF_HDROP`; MS-RDPECLIP carries
  file lists as `CLIPRDR_FILEDESCRIPTOR`, never as `DROPFILES`. Rejected: does not cover this
  format. (See the `rclip-file-desc` README for the verdict on the part it *does* cover.)

## Not implemented (Phase 0)

- **ANSI decoding.** When `fWide == 0` the paths are in the *source machine's* ANSI codepage,
  which is not recorded anywhere in the payload. `Path::Ansi` therefore returns the raw bytes,
  and `chars()` / `to_string_lossy()` return `None` rather than guessing Windows-1252 and
  quietly corrupting every non-Latin path. `Builder::ansi()` likewise takes bytes you encoded.
  Marked `// TODO(phase-1):` in the source; a real fix needs the codepage from `CF_LOCALE` or
  from the platform backend, neither of which exists yet.
- **Lenient parsing of an unterminated array.** Real captures may turn up payloads whose final
  array NUL was never written. Phase 0 rejects them; whether to add a relaxed entry point is a
  decision for when there is a real capture to look at.

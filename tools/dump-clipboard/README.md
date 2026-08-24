# dump-clipboard

Writes every format the system clipboard is currently offering to a directory, one `<flavor>.bin`
plus a `<flavor>.json` sidecar per format, and prints a summary table to stderr. This is how the
captured half of `corpus/` gets made, and it is the tool to reach for whenever a paste misbehaves:
it answers "what did that application *actually* put on the clipboard" without guessing.

```
$ dump-clipboard --list
11 format(s) on the clipboard (the macOS general pasteboard)
  FORMAT                                      FLAVOR     BYTES
  com.apple.webarchive                        -           1796
  public.rtf                                  Rtf         3156
  public.html                                 Html        1508
  public.utf8-plain-text                      PlainText     72
  ...

$ dump-clipboard corpus/macos/safari --app Safari --app-version 18.5 --how "..."
```

`--list` prints without writing. `--primary` reads the PRIMARY selection on X11 and Wayland.
`--app`, `--app-version`, `--how`, `--os` and `--notes` fill in the sidecar fields
`corpus/README.md` requires of a captured fixture; the ones left blank are named on stderr,
because a capture nobody can repeat is not much of a fixture. `--force` overwrites.

**This is a tool, not a codec.** The `no_std` / no-dependency / `forbid(unsafe_code)` rules in
`plan/CONVENTIONS.md` govern `crates/`, not `tools/`: this one links four real OS APIs, uses
`unsafe` where Win32 requires it, and is `publish = false`.

## What is verified and what is not

| Backend | State |
|---|---|
| **macOS** — `NSPasteboard` | **Verified by running.** Built and run on macOS 15.5 (24F74) against real pasteboards from TextEdit, Safari, Finder and Preview; those runs produced `corpus/macos/`. |
| **Windows** — `OpenClipboard`/`EnumClipboardFormats` | Written from the Win32 documentation. Compiles clean for `x86_64-pc-windows-msvc` (`cargo clippy --target … -D warnings`). **Never run.** |
| **X11** — ICCCM selections + INCR | Written from ICCCM 2.0 §2.6–2.7. Compiles clean for `x86_64-unknown-linux-gnu`. **Never run.** |
| **Wayland** — `ext-data-control-v1` / `wlr-data-control` | Written from the two protocol XMLs. Compiles clean for `x86_64-unknown-linux-gnu`. **Never run.** |

"Compiles clean" is a real check — the binding crates' generated types make a wrong method name or
a wrong event shape a compile error — but it is not the same as a live clipboard. Treat the three
untested backends as first drafts until a machine with the relevant display server exercises them.

## Binding crates, and why

- **`objc2` 0.6 + `objc2-app-kit` 0.3** for macOS. The maintained successor to `cocoa`/`objc`,
  and the one with per-class feature gates, so this builds `NSPasteboard` and `NSPasteboardItem`
  and none of the rest of AppKit. Its `NSPasteboard` bindings mark `generalPasteboard`, `types`,
  `pasteboardItems` and `dataForType:` as safe, so the macOS backend contains no `unsafe` at all.
  (`cocoa` is deprecated and `core-foundation` does not cover AppKit.)
- **`windows-sys` 0.61** for Win32, not `windows`. This is raw C API with no COM anywhere —
  `windows-sys` is extern declarations and constants, nothing else, so it costs nothing to build.
  Five feature gates, listed in the manifest.
- **`x11rb` 0.13** for X11, not `x11-dl`/`xcb`. Pure Rust, so nothing links against `libxcb`, and
  it exposes the raw protocol requests (`ConvertSelection`, `GetProperty` with its `delete` flag
  and `bytes_after`) that INCR needs. A convenience wrapper that hides `GetProperty` cannot
  implement INCR at all. Pinned to 0.13 rather than 0.14 only because 0.13 is what the rest of the
  ecosystem is on; the code does not depend on the difference.
- **`wayland-client` 0.31** with `wayland-protocols` (staging) and `wayland-protocols-wlr`.
  Default features off, so the pure-Rust backend is used and `libwayland-client.so` is not needed
  at build time. `rustix` supplies `pipe(2)` and `poll(2)`; it is already in the tree underneath
  `wayland-backend`, so it is not really a new dependency.
- **Nothing for argument parsing or JSON.** Eight flags and one flat object do not justify `clap`
  and `serde`, and the dependency list is more useful as "the four OS APIs and nothing else". The
  sidecar writer emits keys in a fixed order, which `serde_json` would not.

## The parts that are easy to get wrong

Each backend's module doc carries the detail; briefly:

- **macOS: a pasteboard holds *items*, not formats.** `-[NSPasteboard dataForType:]` only reaches
  the first item that offers a type, so a three-file Finder copy reads back as one file. When the
  pasteboard has more than one item this walks `-pasteboardItems` and dumps each separately,
  prefixing filenames `item-00.`, `item-01.`. `corpus/macos/finder/` is that case.
- **Windows: not every handle is memory.** `CF_BITMAP` and `CF_PALETTE` are GDI objects,
  `CF_ENHMETAFILE` is an `HENHMETAFILE`, `CF_OWNERDISPLAY` has no data. `GlobalLock` on any of
  them is undefined, so they are reported as offered and skipped rather than dumped. Also,
  `EnumClipboardFormats` lists the formats Windows *synthesised* (`CF_TEXT` from
  `CF_UNICODETEXT`, `CF_DIB` from `CF_BITMAP`) alongside the ones the application really wrote,
  with nothing to tell them apart.
- **X11: INCR is mandatory.** Anything over roughly 256 KB — and a screenshot always is — arrives
  as a property of type `INCR` whose value is only a lower bound on the size. Skipping the
  handshake does not truncate a large payload, it loses it entirely.
- **Wayland: `wl_data_device` is unreachable here.** A client only receives
  `wl_data_device.selection` while it holds keyboard focus, and focus belongs to a surface; a CLI
  has none. Hence the data-control protocols, which exist for exactly this. **Neither is
  implemented by GNOME's Mutter** — under GNOME this backend says so and stops rather than
  reporting an empty clipboard.

## Filenames

Format identifiers are not filenames: `text/html` has a slash, `Preferred DropEffect` has a space,
`dyn.ah62d4rv4ge8` is legal and opaque. Unsafe characters become `_`, leading dots and trailing
dots are fixed, Windows device names (`NUL`, `COM1`) are escaped, and long identifiers are
truncated. Because that is lossy, a stem already taken gets `-2`, `-3`, … — compared
**case-insensitively**, since APFS and NTFS are, and a corpus that loses a file when checked out on
Windows is not a corpus. The exact native identifier is always in the sidecar's `format` field, so
the sanitised name is a convenience and never the record. Unit tests in `src/naming.rs`.

## Known gaps

- Formats that are offered but not dumpable — a GDI handle, a side-effecting X11 target, a
  promise the source declined — are reported in the table with the reason and written nowhere. The
  corpus convention is one `.json` per `.bin`, so a sidecar with no bytes beside it would be a
  worse lie than a line on stderr. Put them in `--notes` if a capture needs to record them.
- macOS `dyn.` UTIs pass through verbatim. They are a base-encoded legacy OSType/MIME/extension
  and decoding them is worth doing, but `plan/PLAN.md` §4.1 puts that decoder in `rclip-core`,
  where every consumer gets it, not in one tool.
- There is no write path. This reads clipboards; putting a payload *back* is the transport layer's
  job (`plan/PLAN.md` §1, layer 1) and belongs in azul.

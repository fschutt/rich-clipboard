# rich-clipboard

Parsers and serializers for the byte formats behind clipboard and drag-and-drop on Windows,
macOS, X11 and Wayland — built for the [azul](https://github.com/fschutt/azul) GUI framework, but
free of any OS dependency.

Drop a `.lnk` on an app and get a parsed shell link instead of a filename. Paste from Word and
get styled runs instead of flattened text. Copy an image and have the alpha channel survive.

**No OS calls live here.** Transport — `IDataObject`, `NSPasteboard`, ICCCM selections,
`wl_data_offer` — stays in the framework. This workspace is `&[u8] → T` and back, which is what
makes a Windows `.lnk` parser testable on a Mac.

Every codec is `#![no_std]`, `#![forbid(unsafe_code)]`, borrows rather than allocates, and treats
its input as hostile — because a clipboard payload is written by another process and parsed the
instant a user presses Ctrl+V.

- [`plan/PLAN.md`](plan/PLAN.md) — the design plan and phasing
- [`plan/specs.md`](plan/specs.md) — every spec cited, with links
- [`plan/CONVENTIONS.md`](plan/CONVENTIONS.md) — rules for codec crates

## Status

Phase 0. `rclip-core` — the shared flavor registry, bounds-checked reader and error type — is in
place; format crates are being scaffolded against it.

## License

MIT OR Apache-2.0

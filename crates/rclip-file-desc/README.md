# rclip-file-desc

`CFSTR_FILEDESCRIPTORW` — descriptors for files that do not exist on disk. A
`FILEGROUPDESCRIPTORW` is a `UINT cItems` followed by that many 592-byte `FILEDESCRIPTORW`
structs, each naming a file and saying, via `dwFlags`, which of its other fields mean anything.
Parser *and* serializer: this is how Outlook drags an attachment that lives in a database, and
how an application offers "drag this generated PDF into Explorer" without first writing a temp
file — so producing the format is at least as important as consuming it.

Spec: [FILEGROUPDESCRIPTORW][fgd], [FILEDESCRIPTORW][fd], and the CFSTR_FILEDESCRIPTOR /
CFSTR_FILECONTENTS sections of [Shell Clipboard Formats][shell].

[fgd]: https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-filegroupdescriptorw
[fd]: https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-filedescriptorw
[shell]: https://learn.microsoft.com/en-us/windows/win32/shell/clipboard

```rust
use rclip_file_desc::{Builder, FileGroupDescriptor, RawDescriptor, file_attribute};

// Read
let group = FileGroupDescriptor::parse(bytes)?;
for (lindex, d) in group.iter().enumerate() {
    // `lindex` is the FORMATETC::lindex to request this file's CFSTR_FILECONTENTS with.
    println!("{} {:?} bytes", d.file_name_lossy(), d.file_size());
}

// Write
let mut b = Builder::new();
b.push(
    RawDescriptor::new()
        .with_file_size(pdf.len() as u64)
        .with_attributes(file_attribute::NORMAL)
        .with_progress_ui(),
    "report.pdf",
)?;
let payload = b.finish();
```

`no_std`, `forbid(unsafe_code)`, no dependency but `rclip-core`. Parsing borrows the names out
of the caller's buffer; the serializer is behind the `alloc` feature.

## The parts that are easy to get wrong

- **`cItems` is a `u32` from another process.** `0xFFFFFFFF × 592` is a 2.3 TiB read. It goes
  through `Reader::check_count` against the remaining input *before* anything multiplies by 592
  or iterates, and no allocation is ever sized from it. `huge-count.bin` is the fixture.
- **A clear flag is not a zero field.** `dwFileAttributes == 0` with `FD_ATTRIBUTES` clear means
  "not stated"; with it set it means "no attributes". Every optional field is an `Option`
  accessor gated on its flag, so the two cannot be confused, and `RawDescriptor::raw()` is there
  when you want the bytes regardless. Conversely `FD_FILESIZE` with both halves zero is the
  documented way to promise a *zero-length* file, so `file_size()` returning `Some(0)` is a real
  answer. The `two-descriptors.bin` fixture parks `0xDEADBEEF` in `nFileSize*` with `FD_FILESIZE`
  clear to catch a parser that reads the field anyway.
- **There is no padding, and it looks like there should be.** `FILETIME` holds 64 bits but is
  declared as two `DWORD`s, so its alignment is 4, not 8. Every member of `FILEDESCRIPTORW`
  aligns to 4, so the struct is exactly 592 bytes and `fgd` starts at offset **4**, not 8.
  Assuming natural alignment for the timestamps puts the whole array four bytes out.
- **`cFileName` is a fixed 260-unit field, not a string.** It is truncated at the first NUL —
  `Reader::utf16_fixed` does exactly that — and the remaining units are padding that must not
  leak into the name. Producers also put *relative paths* in here (`sub\file.txt`) when
  describing a folder tree; this crate returns what arrived and resolves nothing. Writing is
  capped at 259 units so the terminator has somewhere to go, and the cap counts UTF-16 *units*,
  so an emoji costs two.
- **`FD_LINKUI` is the legacy shortcut marker.** Microsoft: "Before Microsoft Internet Explorer
  4.0, an application indicated that it was transferring shortcut file types by setting
  FD_LINKUI […]. Now, the preferred way […] is to use the `CFSTR_PREFERREDDROPEFFECT` format set
  to `DROPEFFECT_LINK`. However, for backward compatibility with older systems, sources should
  still set the FD_LINKUI flag." So: set both when producing, and prefer
  `CFSTR_PREFERREDDROPEFFECT` when consuming. That format is a bare `DWORD` and lives in
  `rclip_core::flavor::drop_effect`, not here.
- **Timestamps stay raw.** `FILETIME` is exposed as a `u64` of 100ns ticks since 1601-01-01 UTC.
  Turning that into a civil date needs a calendar, and a calendar is not a dependency a
  clipboard codec should carry.

## Prior art

- **[`ironrdp-cliprdr`](https://crates.io/crates/ironrdp-cliprdr)** (0.7.0, 575k downloads) —
  the only real parser out there, and worth reading. `pdu::format_data::file_list` decodes
  `CLIPRDR_FILEDESCRIPTOR` from MS-RDPECLIP, which is the same 592-byte layout: its
  `FIXED_PART_SIZE` is `4 + 32 /* reserved1 */ + 4 + 16 /* reserved2 */ + 8 + 4 + 4 + 520`,
  where `reserved1` is exactly Win32's `clsid`/`sizel`/`pointl` and `reserved2` is exactly
  `ftCreationTime`/`ftLastAccessTime`. Rejected on three counts: it models only 4 of the 10
  flags (RDP declares the rest reserved, Win32 does not), it decodes into owned `String`s under
  `std`, and it drags in `ironrdp-core` + `ironrdp-pdu` + `ironrdp-svc` + `tracing` + `bitflags`
  — an order of magnitude past the ~3-transitive-dep budget in `plan/CONVENTIONS.md` for a
  fixed-layout struct.
- **[`clipboard-win`](https://crates.io/crates/clipboard-win)** (5.4.1) — Windows-only FFI over
  the clipboard API, `std`, and no `FILEGROUPDESCRIPTOR` support at all. Rejected: wrong layer.
- **[`windows`](https://crates.io/crates/windows) / `windows-sys` / `winapi`** — supply
  `FILEDESCRIPTORW` as a `#[repr(C)]` FFI declaration. Using one means an `unsafe` cast over an
  attacker-controlled buffer with `cItems` unchecked, which is the bug this crate exists to make
  unreachable. Rejected: a struct declaration is not a parser.
- **[`lamco-clipboard-core`](https://crates.io/crates/lamco-clipboard-core)** (0.6.1) —
  MIME/Windows format-name mapping, loop detection and chunked transfer for an RDP clipboard;
  no struct codec, and pulls `sha2`, `thiserror` and `tracing`. Rejected: solves a different
  problem.

Searching crates.io for `filegroupdescriptor` returns nothing; `filedescriptor` returns the
unrelated file-handle wrapper crate.

## Not implemented (Phase 0)

- **`CFSTR_FILECONTENTS`.** The bytes of each promised file arrive as a separate format, one per
  descriptor, keyed by `FORMATETC::lindex` and usually delivered as an `IStream`. That is
  transport, not a struct, and belongs in the platform backend. Marked
  `// TODO(phase-1):` in the source.
- **`CFSTR_FILEDESCRIPTOR` (the ANSI `FILEGROUPDESCRIPTORA`).** Same layout with a
  `CHAR cFileName[260]`, so 332 bytes per descriptor, and the same unknowable-codepage problem
  as `CF_HDROP`'s `fWide == 0`. Deferred until there is a real capture that needs it;
  `// TODO(phase-1):` in the source.
- **`dwFileAttributes` interpretation.** The value is passed through verbatim against a handful
  of named `FILE_ATTRIBUTE_*` constants. The full set belongs to `GetFileAttributes` and is not
  worth mirroring here.

# rclip-shell-link

Reads and writes Windows shell links — `.lnk` files, and the `CFSTR_SHELLLINK`
clipboard flavor — per [MS-SHLLINK] revision 10.0 (2025-11-21). `no_std`, no
allocation on the read path, no `unsafe`.

[MS-SHLLINK]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/16cb4ca1-9339-4d0c-a68d-bf1d6cc0f943

```text
SHELL_LINK = SHELL_LINK_HEADER [LINKTARGET_IDLIST] [LINKINFO]
             [STRING_DATA] *EXTRA_DATA
```

| Section | § | Type |
|---|---|---|
| `ShellLinkHeader` | 2.1 | `ShellLinkHeader` |
| `LinkTargetIDList` | 2.2 | `LinkTargetIdList` → [`rclip-idlist`](../rclip-idlist) |
| `LinkInfo` | 2.3 | `LinkInfo`, `VolumeId`, `CommonNetworkRelativeLink` |
| `StringData` | 2.4 | `StringData` |
| `ExtraData` | 2.5 | `ExtraDataBlock`, all eleven signatures |
| writing | — | `ShellLinkBuilder`, behind the `alloc` feature |

## Security

**This parser never resolves or executes anything.** No path resolution, no
filesystem access, no binding an IDList to a namespace extension, no launching.
Bytes in, data out.

That is the whole point. The format has a long CVE history — CVE-2010-2568, the
`.lnk` bug Stuxnet used to spread from USB sticks, and CVE-2017-8464, the same
class again seven years later — and in neither case was the flaw in *parsing* a
`.lnk`. It was in the shell **acting** on what it parsed: loading the icon a
parsed link named, from a path the link chose, at the moment the folder was
displayed, with no user action at all. A shell link is untrusted input that
describes something to execute; keeping the parse and the act apart is the
defence, and this crate is only ever the parse half.

Everything returned is data with nothing attached: `StringData::arguments` is
attacker-chosen text, `icon_location` is an attacker-chosen path, the target
IDList can name any namespace extension on the machine, and a `Darwin` block asks
the Windows Installer to install something. None of it has been validated,
canonicalised or checked against a policy.

## The parts that are easy to get wrong

- **`CountCharacters` counts characters, not bytes**, and with `IsUnicode` set a
  field is `count * 2` bytes long. Worse, that multiply has to happen *after*
  widening to `usize`: done in `u16` it wraps at 32768 and silently yields a
  short string plus a corrupted parse of everything after it. Two published
  crates get this wrong. There are fixtures for both encodings and for the
  overflow.
- **Every offset in `LinkInfo` is relative to the start of its own structure**,
  and the ones inside `VolumeID` and `CommonNetworkRelativeLink` to the start of
  *those*. A wrong base does not produce an error, it produces a plausible string
  from the middle of an adjacent field. Each type here owns the exact slice its
  offsets index.
- **`VolumeLabelOffset == 0x14` is a sentinel, not an offset.** Following it lands
  exactly on the Unicode offset field and yields four bytes that look like a
  two-character label.
- **`BlockSize` includes itself and the signature**, so it is also the
  `ExtraData` walk's stride. A block declaring less than 8 bytes cannot hold a
  signature and is rejected rather than skipped, because skipping it resumes the
  walk inside the next block's fields.
- **`0xA000000A` is unassigned**, and there are only eleven blocks, not twelve.
  An unknown signature round-trips as `ExtraDataBlock::Unknown` rather than
  failing the parse.

## Where the spec and reality disagree

- **`CommonNetworkRelativeLink` Unicode offsets.** MS-SHLLINK gates
  `NetNameOffsetUnicode` on `NetNameOffset > 0x14` but `DeviceNameOffsetUnicode`
  on `DeviceNameOffset > 0x14`. Those cannot both hold: the two fields are at
  fixed positions `0x14` and `0x18`, and with `ValidDevice` clear
  `DeviceNameOffset` MUST be zero, which would leave a hole in the structure.
  This crate gates both on `NetNameOffset`, as the layout requires.
- **`ShowCommand` descriptions.** The spec's prose for `SW_SHOWMAXIMIZED` and
  `SW_SHOWMINNOACTIVE` is transposed — it describes "maximized" as "its window
  is not shown". The constants are right; the prose is not, and is not quoted
  here.
- **`TrackerDataBlock.Length`.** The field definition says MUST be exactly
  `0x58`; §3.1's prose calls the same number a "required minimum size". Since
  `BlockSize` is fixed at `0x60`, exact equality is the only self-consistent
  reading. This crate reads the value and does not enforce either.
- **`StringData`'s 260-character cap** is new in revision 10.0. Links written
  before it exist and exceed it, so the reader does not enforce it; the builder
  does.
- **`VolumeIDSize` MUST be *greater than* `0x10`** — strictly, so `0x11` is the
  floor. Easy to read as `>=`.

## Prior art

- [`lnk`](https://crates.io/crates/lnk) 0.6.4 (89k downloads) — the most complete
  crate, the only published one that also *writes*, and the closest thing to a
  reference implementation. Rejected: `std`-only, allocates throughout, and pulls
  `chrono` + `encoding_rs` + `binrw` + `uuid` and three separate `syn` chains,
  which is more dependency than this whole workspace. It also keeps `ItemID`
  payloads as opaque `Vec<u8>` (no shell item decoding), computes
  `count_characters * 2` in `u16`, mis-maps the unassigned `0xA000000A` to
  `VistaAndAboveIDList`, and fails the whole parse on an unknown `ExtraData`
  signature.
- [`parselnk`](https://crates.io/crates/parselnk) 0.1.1 — **skips the
  `LinkTargetIDList` entirely** (`struct LinkTargetIdList {}`, then seeks past
  it), has the same `u16` overflow, and decodes UTF-16 with `from_ne_bytes`
  rather than `from_le_bytes`. Unmaintained since 2022.
- [`lnk_parser`](https://crates.io/crates/lnk_parser) 0.4.3 — decodes PIDLs via
  `winparsingtools`, but implements **1 of 11** `ExtraData` blocks, and its block
  loop does `vec![0; size - 8]` on a `u32` guarded only against zero, so a
  `BlockSize` of 1-7 wraps to ~4 GiB. Mandatory `serde` + `chrono`.
- [`lnk-core`](https://crates.io/crates/lnk-core) 0.4.1 — the best-written of the
  readers: `forbid(unsafe_code)`, `Option`-returning, and the only one that gets
  the `CountCharacters` widening right. Also decodes only `TrackerDataBlock`, and
  is `std`-only and allocating. Two months old, single maintainer.
- [`mslnk`](https://crates.io/crates/mslnk), `shortcuts-rs` — writers only.
  `lnks`, `ib-shell-item` — COM wrappers around the Win32 API, not byte parsers.

Nothing on crates.io is `no_std` and nothing borrows, so all of them are ruled
out by `plan/CONVENTIONS.md` rule 2 before correctness enters into it. The name
`lnk` is taken, hence `rclip-shell-link`.

The type layout and the spec prose in the doc comments are seeded from
[fschutt/lnk](https://github.com/fschutt/lnk), which parsed the header and had
the rest as types carrying transcribed spec text. Corrected on the way in: its
`bitflags!` values were all `0xFFFFFFFF >> n` rather than `1 << n`, its
`FillAttributes` skipped `FOREGROUND_INTENSITY` and so shifted every background
bit, its `KnownFolderID` was a `u16` where the spec has a 16-byte GUID, it
rejected unknown `LinkFlags`/`FileAttributes` bits and a zero `HotKey`, and its
hand-rolled `FILETIME`-to-calendar conversion confused the 1601 and 1970 epochs.
The `time 0.1` dependency is gone; `FileTime` is a raw `u64` of 100 ns ticks with
`unix_seconds()` for callers who want a date, and no date dependency.

## Reading ANSI `StringData` (the `codepage` feature)

**Done** — was deferred in Phase 0. With `IsUnicode` clear, all five
`StringData` fields are bytes in the writing machine's ANSI code page, and the
file does not say which. The default is unchanged and deliberately lossy:
`ShellStr::to_string_lossy` turns every byte at or above `0x80` into U+FFFD
rather than assuming Windows-1252 and handing back a plausible wrong path.

The optional, default-off `codepage` feature (which forwards to
`rclip-idlist/codepage`) lets a caller that knows the code page say so:

```rust
use rclip_shell_link::{Encoding, ShellLink};

let link = ShellLink::parse(bytes)?;
let enc = link.ansi_encoding().unwrap_or(Encoding::Windows1252);
let name = link.string_data.name.map(|s| s.to_string_with(enc));
```

`ShellLink::console_code_page()` (available with or without the feature) returns
the `CodePage` field of a `ConsoleFEDataBlock` — the only numeric code page a
`.lnk` can carry. Treat it as a **hint**: MS-SHLLINK 2.5.2 defines it as how to
display console text for a target that runs in a console, not as the encoding of
`StringData`, and most ANSI links have no such block at all.
`ansi_encoding()` resolves it through `rclip-codepage` and returns `None` for a
multi-byte page such as 932, because a single-byte table applied to Shift-JIS
produces confident garbage rather than an error.

Fixture: `ansi-string-data-cp1252.bin`.

## Not implemented yet

- `// TODO(phase-3)` **`PropertyStoreDataBlock` is opaque bytes.** Decoding it
  means implementing [MS-PROPSTORE] serialized property storage, which is a
  format of its own. It carries the `AppUserModelID` that taskbar pinning keys
  off, so it will be wanted eventually.
- The builder writes `LinkInfo` with a `VolumeID` and a local base path, but not
  with a `CommonNetworkRelativeLink`. Use `local_path` for local targets,
  `environment_path` for portable ones, and `extra_block` for anything else.
- The builder always writes `StringData` as UTF-16 with `IsUnicode` set. Writing
  code-page strings is still not planned, even though the tables now exist: the
  code page is not recorded in the file, so an ANSI-only link is only reliably
  readable on a machine configured like the one that wrote it. Reading them is a
  compatibility obligation; writing them would be creating the problem.
- No jump list (`.automaticDestinations-ms`) support. Those are CFB containers of
  shell links and belong in their own crate.

[MS-PROPSTORE]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-propstore/39ea873f-7af5-44dd-92f9-bc1f293852cc

## Fixtures

`corpus/synthetic/rclip-shell-link/`. `full-featured.bin` exercises every section
at once, including an `ExtraData` block with the unassigned `0xA000000A`
signature. The malformed ones each assert a specific `ErrorKind`:
`truncated-header`, `bad-clsid`, `bad-header-size`, `string-count-past-end`,
`extra-block-too-small`, `link-info-too-small`, `id-list-size-past-end`.

Two of the tests are randomised rather than fixture-driven — one walks
`ExtraData` over random trailing bytes, the other bit-flips and truncates the
valid fixtures — asserting only that nothing panics and that every walk makes
progress. That is what a fuzzer would check, and Phase 0 has no fuzzer.

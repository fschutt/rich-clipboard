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

## The shared shortcut type

`ShellLink::target()` answers the one question `.url`, `.webloc`, `.desktop` (`Type=Link`) and
`text/uri-list` also answer, in the same vocabulary: `rclip_core::shortcut::ShortcutTarget`,
re-exported here as `rclip_shell_link::ShortcutTarget`. See `plan/PLAN.md` §4.10.

A `.lnk` is the awkward member of that family. The other four state their destination once, as a
string; a shell link states it up to four times, in four structures, in two encodings, and calls
the authoritative one an `ITEMIDLIST` — which is a binary shell namespace path and not text at
all. So `target_candidates()` enumerates the destination *strings* the file carries, most absolute
first:

| Order | Source | Example |
|---|---|---|
| 1 | `LinkInfo` `LocalBasePath` (2.3) | `C:\Users\me\notes.txt` |
| 2 | `CommonNetworkRelativeLink` `NetName` (2.3.2) | `\\fileserver\public` |
| 3 | `EnvironmentVariableDataBlock` (2.5.4) | `%windir%\system32\cmd.exe` |
| 4 | `StringData` `RELATIVE_PATH` (2.4) | `.\notes.txt` |

`target()` is the first of those that can be borrowed as a `&str`, classified. It returns `None`
for a UTF-16 field and for an ANSI field holding a byte above `0x7F`, because both need
re-encoding and re-encoding allocates — which is most modern links, since Windows sets
`IsUnicode`. That is deliberate: the candidate list hands back the `ShellStr` either way, and
`to_string_lossy` behind the `alloc` feature is where a caller that wants a `String` goes.

Two more things it does not do. The target IDList is not a candidate — walk it with
`LinkTargetIdList::items` — and `LocalBasePath` is skipped when `CommonPathSuffix` is non-empty,
because MS-SHLLINK 2.3 forms the full path by concatenating the two and returning the base alone
would be a confidently wrong path.

## The property store (`src/propstore.rs`)

**Done** — was `// TODO(phase-3)`. `PropertyStoreDataBlock` (2.5.7) is no longer
opaque bytes: its payload is an [MS-PROPSTORE] serialized property storage and
this crate decodes it.

```rust
let link = ShellLink::parse(bytes)?;
link.app_user_model_id();                  // the one-liner
link.property_store()                      // or address it explicitly
    .and_then(|s| s.get(&FMTID_APP_USER_MODEL, PID_APP_USER_MODEL_ID));
```

`System.AppUserModel.ID` is why this is worth having. It is the string Windows
groups taskbar buttons by, the one a pinned shortcut relaunches through, and the
one a Jump List hangs off — the only application identity a `.lnk` carries that
is not a file path. Format ID `{9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}`, property
`5`. It is also entirely the writer's choice, so it is an identity *claim*:
nothing stops a shortcut from naming another application's AppUserModelID, which
is exactly how one gets itself grouped under someone else's button.

Nine `VT_*` types are decoded — the ones that turn up in a shell link:

| Covered | Not covered |
|---|---|
| `VT_EMPTY`, `VT_NULL`, `VT_I4`, `VT_UI4`, `VT_BOOL`, `VT_FILETIME`, `VT_CLSID`, `VT_LPWSTR`, `VT_BSTR` | everything else, including every `VT_VECTOR`/`VT_ARRAY` and `VT_LPSTR` |

Anything else becomes `PropertyValue::Unsupported { property_type, data }` with
its raw payload, and the walk carries on to the next property — values are
length-delimited by `ValueSize`, which is what makes skipping one possible
without understanding it. Refusing to guess is the point: a
`VT_VECTOR | VT_LPWSTR` read as a `VT_LPWSTR` yields a plausible string that is
not the property's value. `PropertyValue::decode` reports `ErrorKind::Unsupported`
for a caller that wants the error rather than the variant.

Three details that a hand-rolled reader gets wrong:

- **`Version` is the only hard check.** `0x53505331` — `"1SPS"` on the wire. A
  storage that says anything else is rejected with `ErrorKind::BadMagic`,
  because otherwise a mis-sized block reads as a storage whose format ID is
  sixteen bytes of something else, and values under a wrong format ID are worse
  than no values.
- **`Reserved` comes *before* the name**, not after it: 2.3.1 is `ValueSize`,
  `NameSize`, `Reserved`, `Name`. Reading it after shifts every value in the
  storage by one byte.
- **`UnicodeString`'s `Length` counts characters, not bytes.** Reading it as a
  byte count halves every string. `CodePageString`'s `Size` *is* bytes.

Whether a storage's values are string-named is a property of the *storage*: it
is the integer-named form unless the format ID is
`{D5CDD505-2E9C-101B-9397-08002B2CF9AE}`. There is no per-value discriminator,
so a value cannot be read out of context.

The same structure appears inside a PIDL — `rclip_idlist::UsersPropertyView`
hands back an MS-PROPSTORE blob — and `PropertyStore::new` takes any `&[u8]`, so
those bytes go straight through this decoder for a caller that has both crates.

## Not implemented yet

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

The four property-store fixtures all expect `ok`, and deliberately:
`property-store-bad-version` and `property-store-value-size-too-small` are
well-formed *shell links* whose *store* is broken, so `ShellLink::parse` and the
`ExtraData` walk both succeed and the failure only appears once you walk the
store. Their sidecars name the `ErrorKind` in `notes` and the crate's own tests
assert it — `BadMagic` for the version, `BadLength` for the value size.
`property-store-mixed-types` carries a `VT_VECTOR | VT_LPWSTR` between two
decodable properties, which is what proves an unknown type costs one value and
not the storage.

Two of the tests are randomised rather than fixture-driven — one walks
`ExtraData` over random trailing bytes, the other bit-flips and truncates the
valid fixtures — asserting only that nothing panics and that every walk makes
progress. That is what a fuzzer would check, and Phase 0 has no fuzzer.

# rclip-idlist

Parses Windows shell `ITEMIDLIST`s (PIDLs) and the `CIDA` array behind the
`CFSTR_SHELLIDLIST` ("Shell IDList Array") clipboard format. `no_std`, no
allocation on the read path, no `unsafe`.

A PIDL is how the Windows shell names a thing, and not every thing in the shell
namespace has a path — the Recycle Bin, a camera, a mail message and the inside
of a zip file all have PIDLs and none of them have filenames. When Explorer puts
a selection on the clipboard, a PIDL is the only representation that covers all
of them, which is why this crate exists separately from `rclip-dropfiles`.

## Specs

| Part | Source |
|---|---|
| `CIDA` | [`ns-shlobj_core-cida`](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-cida), [Shell Clipboard Formats](https://learn.microsoft.com/en-us/windows/win32/shell/clipboard) |
| `ITEMIDLIST` / `SHITEMID` | [`ns-shtypes-itemidlist`](https://learn.microsoft.com/en-us/windows/win32/api/shtypes/ns-shtypes-itemidlist) |
| Item **contents** | **Not documented by Microsoft.** [libfwsi — Windows Shell Item format](https://github.com/libyal/libfwsi/tree/main/documentation), cross-checked against the `libfwsi` C sources |

The outer wrapper is public API; the contents of `abID` are private to whichever
namespace extension owns the item and are reverse-engineered. Where the libfwsi
*document* and the libfwsi *code* disagree, this crate follows the code and says
so at the field in question — see `Volume` (the `0x2E` split) and
`FileEntryExtension` (the field at `body[8..10]` is an offset, not a length).

## The two things this crate is careful about

**`cb` below two.** `cb` counts its own two bytes *and* is the walk's stride, so
a `cb` of zero that is not meant as a terminator makes a naive walk spin forever
and a `cb` of one makes it reparse the same bytes at a sliding offset. `cb == 0`
is treated as the terminator it is defined to be; `cb == 1` is
`ErrorKind::BadLength`. There is a fixture for each, plus an exhaustive test over
all 65 536 values of the first size field and a 2 000-round randomised smoke test.

**Unknown items are not failures.** Item decoding is infallible. Anything
unrecognised comes back as `ShellItem::Unknown { class, raw }` with its bytes
intact, and the items around it still parse. A PIDL from a shell extension nobody
has reverse-engineered must not be able to break a paste.

Structural parsing is strict in the other direction: `aoffset` entries are
validated against the buffer, `cidl` is checked against the bytes actually
present before it sizes anything, and a truncated item is an error rather than a
silent stop.

## Security

This crate reads bytes and returns data. It never resolves a PIDL, binds to a
namespace extension, touches the filesystem or reads the registry. `display_name`
returns a **label**, not a path component: nothing has validated it, and it can
contain a path separator, `..`, or a right-to-left override.

## Prior art

Nothing on crates.io is `no_std`, and nothing borrows — every crate below
allocates `Vec`/`String` during parse, which rules them all out under
`plan/CONVENTIONS.md` rule 2 regardless of quality. Recorded anyway, because the
`cb` handling is the interesting axis:

- [`shellitem`](https://crates.io/crates/shellitem) 0.2.3 — the only dedicated
  PIDL crate, and the only one that gets the `cb` walk fully right (`cb == 0`
  terminator, `cb < 3` break, `checked_add` bounds, zero panics). Rejected for
  `std` + allocation, and for pulling a ~780 KB `forensicnomicon-data` catalog in
  just for GUID names. Worth reading.
- [`winparsingtools`](https://crates.io/crates/winparsingtools) 2.1.4 — the shell
  item engine behind `lnk_parser`. Handles unknown classes gracefully, but has
  no `cb < 2` rejection, aborts the whole list on one malformed item, and its
  public `ShellItem::from_buffer` reaches a `vec![0; (size - 2) as usize]` that
  underflows on `cb == 1`. Mandatory `serde_json` + `chrono` + `encoding_rs`.
- [`lnk`](https://crates.io/crates/lnk) 0.6.4 — walks the `SHITEMID` chain but
  keeps every payload as an opaque `Vec<u8>`, so it decodes no shell items at
  all. Its surrounding loop does `bytes_to_read -= item_id.size()` on two `u16`s
  with no check that the size fits, which underflows.
- [`parselnk`](https://crates.io/crates/parselnk) 0.1.1 — skips the
  `LinkTargetIDList` entirely (`struct LinkTargetIdList {}`); nothing to reuse.
- [`lnk-core`](https://crates.io/crates/lnk-core) 0.4.1 — delegates PIDLs to
  `shellitem`. Same rejection.

We wrote our own because the workspace needs a borrowing, allocation-free,
`no_std` parser that degrades to `Unknown` instead of failing, and no published
crate is more than two of those four things.

## Code pages (the `codepage` feature)

**Done** — was `// TODO(phase-3)`. `ShellStr::Ansi` holds bytes in the writing
machine's ANSI code page, which is not recorded in the payload. The default
behaviour is unchanged and deliberately lossy: `chars()` reports
`ErrorKind::Unsupported` for every byte at or above `0x80` and `to_string_lossy()`
substitutes U+FFFD, because a wrong path that looks right is worse than one that
is visibly lossy.

With the optional, default-off `codepage` feature a caller that *knows* the code
page — from `CF_LOCALE`, from a `.lnk` `ConsoleFEDataBlock`, from the user — can
say so:

```rust
use rclip_idlist::{Encoding, ShellStr};

let name: ShellStr<'_> = /* … */;
name.chars_with(Encoding::Windows1252);            // borrows, allocates nothing
name.to_string_with(Encoding::Windows1252)?;       // Err at an undefined byte
name.to_string_lossy_with(Encoding::Windows1252);  // U+FFFD only where undefined
```

The feature pulls in `rclip-codepage` (~3 KB of tables) and nothing else; with it
off, the crate links no tables at all. `file-entry-ansi-cp1252.bin` is the
fixture: the same name reads `Grüße.txt` under Windows-1252 and `GrьЯe.txt` under
Windows-1251, which is why the code page is a parameter and never a guess.

## Not implemented yet

- `// TODO(phase-3)` Signature-based item recognition — MTP devices, control
  panel categories, users-property-view items, delegate folders, and the
  compressed-folder heuristics. libfwsi probes a set of 32-bit signatures at
  fixed offsets *before* falling back to the class byte; these all land in
  `ShellItem::Unknown` here.
- `// TODO(phase-3)` Control panel items (class `0x71`) are not decoded.
- `// TODO(phase-3)` Pre-XP file entries carry a *secondary* (8.3) name after the
  primary one instead of an extension block. Not read.
- `// TODO(phase-3)` The URI item's FTP data block (`>= 36` bytes: a `FILETIME`
  and three length-prefixed strings, one of which is a cleartext password) is
  kept as opaque bytes.
- `// TODO(phase-3)` Extension blocks other than `0xBEEF0004` are located,
  bounded and handed back with their signature and body, but not decoded.
- The `alloc` builder only synthesises root folder items. Forging a *file entry*
  PIDL is not a layout problem: the shell binds a PIDL by handing the bytes back
  to the namespace extension that owns them, so a hand-built one resolves to
  something unintended or not at all. Copy PIDL bytes you were given instead —
  that is what `push_raw` is for.

## Fixtures

`corpus/synthetic/rclip-idlist/`. `file-entry-beef0004-v3.bin` is byte-for-byte
libfwsi's own `fwsi_test_file_entry_values_data1` vector, which pins every offset
in the `0xBEEF0004` layout. The malformed ones (`cb-one-bomb`,
`item-runs-past-end`, `odd-trailing-byte`, `cida-child-offset-past-end`,
`cida-count-too-large`) each assert a specific `ErrorKind`; `cb-zero-bomb` asserts
a clean stop, since hanging is the failure mode it guards.

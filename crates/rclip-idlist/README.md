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

## Signature-based items (`src/signature.rs`)

**Done** — was `// TODO(phase-3)`. Several namespace extensions write a class
type indicator of `0x00`, which means nothing, and identify themselves with a
32-bit signature a few bytes into the body instead. `ShellItem::parse` therefore
runs `signature::recognise` *before* the class byte, in libfwsi's order:

| Item | How it is recognised |
|---|---|
| `ShellItem::DelegateFolder` | class identifier `{5E591A74-…}` 32 bytes before the end of the item |
| `ShellItem::MtpVolume` | `0x10312005` at `abID[4..8]` |
| `ShellItem::MtpFileEntry` | `0x07192006` at `abID[4..8]` |
| `ShellItem::UsersPropertyView` | one of six signatures at `abID[4..8]` |
| `ShellItem::CompressedFolder` | the punctuation of its formatted timestamp |

Two things about that order are worth knowing. It is not free: the probes read
fixed offsets on items of *every* class, so an item whose `FileSize` or FAT
timestamp happened to spell a signature would be reclassified — libfwsi has the
same property, the values make it a curiosity rather than a risk, and the cost
of a wrong guess is one breadcrumb segment. And the delegate folder is *not*
recursive: `DelegateFolder::inner_item` goes through `parse_no_delegate`, so a
PIDL that nests wrappers a thousand deep cannot turn into a thousand stack
frames.

Three units differ between neighbouring layouts and each one is a silent
corruption if read wrong, so they are called out at the field: the MTP string
lengths count UTF-16 **characters including** the terminator, the compressed
folder's count characters **excluding** it, and the FTP block's count **bytes**.

`UsersPropertyView::property_store` is handed back as raw bytes. It is an
MS-PROPSTORE serialized property storage — byte-identical in structure to a
`.lnk` `PropertyStoreDataBlock` payload, which `rclip-shell-link` decodes — but
codec crates in this workspace do not depend on each other and a PIDL parser has
no business linking a shell link parser.

## Control panel items, pre-XP file entries, FTP URIs

All three were `// TODO(phase-3)` and all three are done.

- **Class `0x71`** is a GUID and nothing else, so `Guid::control_panel_name`
  carries libfwsi's 76-entry identifier table. It is deliberately a *separate*
  table from `well_known_name`: the two namespaces overlap and disagree —
  `{BD84B380-…}` is `Fonts` as a shell folder and `Font Folder` as a control
  panel item — so looking one up in the other is not merely incomplete, it is
  wrong.
- **Pre-XP file entries** carry the 8.3 name where a newer shell writes an
  extension block. Nothing in the item says which layout it is; libfwsi decides
  by looking at the `u16` after the primary name and asking whether it could be
  an extension block size, and `FileEntry::is_pre_xp` reproduces that
  look-ahead, alignment padding and the `0xB1` SolidWorks exclusion included.
- **The URI item's `>= 36` byte FTP block** decodes into `FtpData`. It contains
  a **cleartext password**, so `FtpData`'s `Debug` prints `<redacted>` for it.
  That is a courtesy against accidental logging and not a boundary: the field is
  public and the bytes are still in `Uri::data`.

## Not implemented yet

- `// TODO(phase-4)` Extension blocks other than `0xBEEF0004` are located,
  bounded and handed back with their signature and body, but not decoded.
- Signature-based items libfwsi knows and this crate does not: Acronis TIB
  files, CDBurn, Game Folder, Web Sites, control panel *categories* and control
  panel CPL files. Each is a fixed signature away, and each was left out for the
  same reason: none of them carries a display name that a breadcrumb wants, so
  the whole benefit would be a better `Debug` line. They stay `Unknown` with
  their bytes intact.
- The delegate folder's inner data is handed back as the wrapped item's `abID`.
  libfwsi additionally re-aligns it by four bytes for four specific folder
  identifiers and not at all for the search folder; `inner_item()` does not,
  because the re-alignment is a per-extension quirk and the raw `inner` slice is
  right there for a caller that knows which extension it is looking at.
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

The Phase-3 additions are one fixture per newly recognised item —
`control-panel-item`, `mtp-volume`, `mtp-file-entry`, `users-property-view`,
`compressed-folder-win10`, `delegate-folder`, `uri-ftp`, `file-entry-pre-xp` —
plus `mtp-volume-length-bomb`, which declares `0xFFFFFFFF` characters of name in
a sixty-byte item. That one is the fixture that matters: it expects `ok`,
because item parsing in this crate never fails, and it asserts that the string
comes back absent rather than truncated and that the characters-to-bytes
multiply does not overflow on a 32-bit target.

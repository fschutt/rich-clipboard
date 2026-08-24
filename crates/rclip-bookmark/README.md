# rclip-bookmark

Reader for macOS **`BookmarkData`** — the `book` / `alis` blob that
`NSURL.bookmarkData` returns, that modern Finder alias files contain, and that
turns up on the pasteboard whenever an alias is dragged. `#![no_std]`,
`#![forbid(unsafe_code)]`, borrows from the caller's buffer and allocates
nothing outside the optional `alloc` feature.

**Why this rather than a `file://` URL:** a bookmark records the target's
catalog node ID and its volume's UUID *alongside* the path components, which is
what lets macOS resolve it after the file has been moved or renamed. A URL on
the pasteboard breaks the moment the user drags the file to another folder; a
bookmark does not.

## Spec

There isn't one. Apple has never documented the format. This implementation
follows three independent reverse-engineering efforts and a set of real
captures:

- ★ [mac_alias — Bookmark format](https://mac-alias.readthedocs.io/en/latest/bookmark_fmt.html),
  and its reference implementation `mac_alias/bookmark.py` (v2.2.3), which is
  the most precise of the three and the one this crate is checked against.
- [mikeymikey — Apple's BookmarkData exposed](http://michaellynn.github.io/2015/10/24/apples-bookmarkdata-exposed/)
- [Mother's Ruin — URL bookmarks and security scoping](https://www.mothersruin.com/software/Archaeology/reverse/bookmarks.html)
- `corpus/synthetic/rclip-bookmark/corefoundation-file.bin`, produced by
  CoreFoundation on macOS 15.5.

### Layout

```
header    magic 'book'|'alis' │ u32 total size │ u32 version │ u32 header size │ reserved
@hdrsize  u32 offset of the first TOC
TOC       u32 size-8 │ u32 0xFFFFFFFE │ u32 id │ u32 next TOC │ u32 count │ entries[]
entry     u32 key │ u32 record offset │ u32 reserved
record    u32 length │ u32 type │ payload, padded to a 4-byte boundary
```

Three things are easy to get wrong:

1. **Every offset is relative to the end of the header**, not to the start of
   the buffer, and the header length is a *field*. It is 48 bytes on everything
   through macOS 15 and 64 bytes on macOS 26, where the prolog grew a team
   identifier. Hardcoding 48 works right up until it doesn't.
2. **`0x0400` date records are big-endian.** Everything else in the format is
   little-endian. Read a date the wrong way round and you do not get an error,
   you get a plausible `f64` about 10^300 seconds from the epoch. `rclip-core`
   has `Reader::f64_be` for exactly this field.
3. **TOC entry key bit 31.** When `key & 0x80000000` is set, the low 31 bits are
   an *offset to a string record* naming the key. Miss it and a named key looks
   like a nonsense two-billion number that never matches anything.

### Where the references disagree

| Point | `mac_alias` | mikeymikey | Mother's Ruin | What this crate does |
|---|---|---|---|---|
| TOC header shape | `u32` size-8, `u32` magic `0xFFFFFFFE`, `u32` id, `u32` next, `u32` count | `u32` len, `u16` type `0xFEFF`, `u16` flags `0xFFFF`, `u32` level, `u32` next, `u32` count | sentinel `0xfffffffe`, unnamed fields, next, count | `mac_alias`. mikeymikey's `0xFEFF` is `0xFFFFFFFE` read as two little-endian `u16`s with the halves swapped; the byte sequence all three describe is identical |
| TOC entry shape | `u32` key, `u32` offset, `u32` reserved-0 | `u16` type, `u16` flags, `u64` offset | key, value offset, "unknown, possibly flags" | Twelve bytes either way. Read as `mac_alias` describes; the third word is read and discarded rather than validated, since nobody can say what it means |
| `0x0303` / `0x0304` | `SInt32` / `SInt64` (signed — they are `CFNumberType` values) | not addressed | `UInt32` / `UInt64` | Signed. It only matters for `0x2012` volume size above 8 EiB |
| `0xc001` | index of the containing folder within the path array | — | number of path components below the user's home directory | Exposed as the raw integer under `key::CONTAINING_FOLDER`; the two readings coincide in the common case and neither is verifiable from the bytes |
| Prolog length | 48, fixed | 48, fixed | 48 **or 64** — 64 with version `0x10050000` (macOS 26+), carrying a team identifier | The header-size field is read and used as the offset base, so both work |
| `0x2011` volume UUID | "stored (perversely) as a string" | — | "string, not UUID type" | Agreed, and confirmed by the real capture: a 36-character dashed string in a `0x0101` record, not a `0x0801` UUID record |

Two keys that every writeup lists — `0x1003` target URL and `0x1020` target
filename — are **absent** from real CoreFoundation output for a plain file
target. The captured fixture asserts their absence so the surprise does not have
to be rediscovered.

## Prior art

Searched crates.io for BookmarkData, alias-record and Finder-alias parsers.

- **No Rust crate implements this format.** `cargo search` for
  `bookmarkdata` / `mac alias` / `alias record` turns up bookmark *managers*
  (`bkmr`, `bookmark-cli`, `mark_recall`), `alias_macros` (a proc macro for type
  aliases), and `macos-unifiedlogs` (a different Apple format). Nothing parses
  `book` / `alis`.
- **`mac_alias` (Python, v2.2.3)** is the de-facto reference and the crate this
  implementation is validated against — the synthetic fixtures are round-tripped
  through it. Not usable as a dependency, and it has two liveness bugs this
  crate deliberately does not reproduce: a TOC whose `next` offset points back
  at itself loops forever, and a container whose element offset points at itself
  recurses until the interpreter's stack limit. Both have fixtures here.
- **`plist`** does not apply: `BookmarkData` is not a property list, despite
  looking like one from a distance.

Verdict: written from scratch, which was never in doubt — there was nothing to
reuse.

## Security

Every offset in this format comes off the wire and is attacker-controlled: the
first-TOC pointer, each TOC's `next`, every entry's record offset, and every
element of every array and dictionary. Three guards:

- **All offsets are validated** against the bookmark's *declared* size — not the
  length of the slice handed in — through the single `Bookmark::abs` choke
  point. Trailing bytes after the declared size are tolerated (bookmarks arrive
  embedded in pasteboard items) but are not reachable through any offset.
- **Depth is bounded** at `rclip_core::MAX_DEPTH`. Containers hand their
  elements `depth + 1`, so an offset that points back at its own container costs
  one level per hop and stops. `corpus/synthetic/rclip-bookmark/cyclic-array.bin`
  is that case.
- **A node budget bounds total work** in `Bookmark::validate`. Depth alone is not
  enough: eight nested arrays of eight shared references each fit in 400 bytes,
  nest only eight levels, and still cost 8^8 resolutions to walk.
  `fanout-bomb.bin` is that case. The TOC chain gets the same treatment, because
  a self-referential `next` pointer is an infinite loop with no recursion in it
  for a depth counter to catch.

Field access is lazy, so none of the above costs anything until a caller asks
for a value. `Bookmark::validate` is the opt-in full walk.

## `CFNumberType`

A `0x03xx` number record's low byte is a `CFNumberType`, and the whole
enumeration from CoreFoundation's `CFNumber.h` is decoded:

| Subtype | | Width |
|---|---|---|
| 1–6 | `SInt8` … `Float64` | fixed by the constant |
| 7–9, 11–13 | `char`, `short`, `int`, `long long`, `float`, `double` | fixed by C |
| 10, 14, 15, 16 | `long`, `CFIndex`, `NSInteger`, `CGFloat` | **4 or 8, per the writer** |

The last row is the interesting one. Those four name a C or platform type whose
width follows the data model of the process that wrote the record — four bytes
from a 32-bit process, eight from a 64-bit one — so the subtype alone does not
say how many bytes to read. The **record's own length field** does, and it is
authoritative: the same encoder wrote it, in the same process, from `sizeof` the
value. So those four dispatch on the payload length.

Getting that wrong is not an error, it is a wrong number: assuming eight bytes
for a four-byte `long` reads the next record's length word as the value's high
half. Above `kCFNumberMaxType` (16) there is no `CFNumberType`, and the record
comes back as `Value::Unknown` with its payload intact rather than guessed at.

Nothing captured exercises 7–16, because CoreFoundation normalises to 1–6 when
it encodes a bookmark. `corpus/synthetic/rclip-bookmark/cfnumber-subtypes.bin`
covers all of them, with 10, 14, 15 and 16 present at both widths.

## What is not implemented

- **No serializer.** `PLAN.md` scopes writers to the formats where round-tripping
  is load-bearing (`shell-link`, `cf-html`, `dropfiles`); a bookmark is only ever
  read. Phase 4 left it that way on purpose: a bookmark's whole value is that
  macOS can *resolve* it, and a writer would have to invent a CNID and a volume
  UUID that no filesystem agreed to — a blob that looks like a bookmark and
  resolves to nothing.
- **The legacy `alias` record** (the pre-10.6 `AliasHandle` structure, not the
  `alis`-signed bookmark) is a different format and is not handled. Key `0xFE00`
  carries one as opaque bytes when present.
- **Sandbox extension tokens** (`0xF080` / `0xF081`) are returned as opaque
  bytes and deliberately not parsed. They are capability tokens authenticated by
  an HMAC keyed to the machine and the current boot; interpreting or reissuing
  one is not this crate's business.
- **Nothing is resolved.** No filesystem access, no CNID lookup, no symlink
  following. `Bookmark::target_path` (behind `alloc`) reconstructs a path string
  from the stored components and nothing more.

## Fixtures

`corpus/synthetic/rclip-bookmark/`, each `.bin` with a `.json` sidecar.

| Fixture | Expect | What it pins down |
|---|---|---|
| `corefoundation-file.bin` | ok | **Real capture** — `NSURL.bookmarkData` on macOS 15.5 |
| `url-and-filename.bin` | ok | Minimal `book`: `0x1003` URL and `0x1020` filename |
| `path-components.bin` | ok | `0x1004` array of path components |
| `date-alis.bin` | ok | `alis` magic; big-endian `0x0400` dates; `0x1010` flags |
| `named-key-nested.bin` | ok | Bit-31 string key; shared subtrees are not cycles |
| `cfnumber-subtypes.bin` | ok | Every `CFNumberType`, with 10/14/15/16 at both widths |
| `cyclic-array.bin` | error | Self-referential element offset → `DepthLimit` |
| `fanout-bomb.bin` | error | 8^8 nodes from 416 bytes → `TooLarge` |
| `toc-self-loop.bin` | error | `next` points at the same TOC → `Malformed` |
| `offset-past-end.bin` | error | Record offset past the buffer → `BadOffset` |
| `header-size-overruns.bin` | error | Header size larger than the blob → `BadLength` |
| `bad-magic.bin` | error | → `BadMagic` |
| `truncated-header.bin` | error | → `UnexpectedEof` |

Four of the `ok` synthetic fixtures are cross-checked against `mac_alias` 2.2.3,
which decodes all of them to the values the sidecars claim. `cfnumber-subtypes`
is not among them: `mac_alias` stops at `CFNumberType` 6 as well, so it is the
thing being tested rather than an oracle for it.

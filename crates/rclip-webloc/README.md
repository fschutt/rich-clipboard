# rclip-webloc

Reader for macOS **`.webloc`** and **`.inetloc`** internet location files — a
property list whose single dictionary has the key `URL`, plus `URLName` on a
`.inetloc`. Reads all three encodings that occur in the wild: XML plists,
`bplist00` binary plists, and the pre-OS X **resource fork** with its `url `
resource. `#![no_std]`, `#![forbid(unsafe_code)]`, borrows from the caller's
buffer, no dependencies beyond `rclip-core`.

## Spec

There is no specification for `.webloc` itself; the file is simply a plist and
the convention is the `URL` key. References:

- [Eclectic Light — data formats used in textClipping, webloc and mailloc](https://eclecticlight.co/2025/12/30/data-formats-used-in-textclipping-webloc-and-mailloc-files-1994-2025/)
  for the history and the legacy resource-fork form.
- Apple's binary property list format is defined by CoreFoundation's
  `CFBinaryPlist.c`, which is open source; the trailer and marker layout below
  comes from it.
- `corpus/synthetic/rclip-webloc/finder-created.webloc`, written by Finder on
  macOS 15.5, and `finder-binary.webloc`, the same file through
  CoreFoundation's own binary writer.

### Resource fork layout

The legacy form, read by the `rsrc` module. From *Inside Macintosh: More
Macintosh Toolbox*, "Resource File Format", cross-checked against the Kaitai
Struct [`resource_fork.ksy`](https://formats.kaitai.io/resource_fork/) and
against the captured fork byte for byte. **Big-endian throughout**, like the
binary plist and unlike every Win32 structure in this workspace.

```
header   16 bytes at 0:  u32 data_offset │ u32 map_offset
                         u32 data_len    │ u32 map_len
         then 112 bytes reserved for the system and 128 for the application

data     at data_offset: one block per resource, u32 length then that many bytes

map      at map_offset:  16 reserved header copy │ 4 next-map handle
                         │ 2 file ref │ 2 attributes
                         │ 2 offset to the type list │ 2 offset to the name list

type     u16 number of types MINUS ONE, then 8 bytes per type:
list       4 type code │ u16 count minus one │ u16 offset to its reference
           list, measured from the start of the type list

ref      12 bytes per resource: i16 id │ u16 name offset (0xFFFF = unnamed)
list       │ u8 attributes │ u24 offset into the data area │ u32 reserved

names    Pascal strings: one length byte, then that many bytes
```

The two *minus one* counts are the trap. A type list that says `0` holds one
type, and one that says `0xFFFF` holds **none** rather than 65 536 — wrapping,
not saturating, which is how an empty fork is expressible.

The three resources Finder writes are `url ` (the URL; note the trailing space,
type codes are exactly four characters), `TEXT` (the URL again, so that dragging
the file somewhere expecting text produces something), and `drag` (the Drag
Manager flavor list naming the other two). There is no `urln`: asking Finder to
write an internet location file with a name puts the name in the *filename*, and
files written for `http`, `mailto`, `ftp`, `afp`, `file` and `news` URLs on
macOS 15.5 all carried exactly those three resources. So `Webloc::url_name` is
always `None` for this form rather than guessed at from `TEXT`.

A resource fork is a separate stream and is not in the file's bytes: on macOS it
is `<file>/..namedfork/rsrc`, in an archive it is an AppleDouble sidecar, and on
the clipboard it does not travel at all. Hand `Webloc::parse` whichever stream
you have; it works out which of the three encodings it is.

### Binary plist layout

```
header    8 bytes: "bplist" + version, "00"
objects   one after another, each introduced by a marker byte
table     num_objects offsets, offset_size big-endian bytes each
trailer   last 32 bytes: 5 unused │ sort version │ offset_size │ ref_size
                         │ u64 num_objects │ u64 top_object │ u64 table_offset
```

A marker is a type in the high nibble and a count in the low one; a low nibble
of `0xF` means the real count follows as an integer object, which every string
longer than fifteen bytes takes. Everything is **big-endian**, including `0x6n`
UTF-16 strings — the opposite of every other format in this workspace, so
`rclip_core::Utf16Le` is the wrong tool and this crate decodes UTF-16BE itself.

## Prior art

The interesting call here, since `plist` is mature and correct and the plan
initially recommended reusing it.

- **[`plist`](https://crates.io/crates/plist) 1.9 — rejected.** Correct, well
  maintained, and far too much crate for the job. `cargo tree` on a scratch
  project gives **thirteen transitive dependencies**: `base64`, `indexmap`
  (`equivalent`, `hashbrown`), `quick-xml` (`memchr`), `serde` (`serde_core`),
  `time` (`deranged`, `num-conv`, `powerfmt`, `time-core`). `CONVENTIONS.md`
  draws the line at about three and calls out `serde` by name. It is also
  `std`-only, which rules it out for a `#![no_std]` codec regardless of the
  dependency count. Against that cost: what this crate needs is *one string
  value under one known key*. The binary plist reader that satisfies that is
  ~230 lines including its tests, and pulling a string out of an XML plist is a
  narrow scan. Written from scratch.
- **[`bplist`](https://crates.io/crates/bplist) 0.1.0 — rejected.** Pulls
  `serde`, single 0.1.0 release, and `std` by default. Same trade as above with
  less maturity behind it.
- **[`apple-plist`](https://crates.io/crates/apple-plist) 1.0 — rejected.** The
  most complete of the alternatives (XML, binary, OpenStep, GNUStep) and
  therefore the least proportionate. `serde` again, plus `crc32fast` for the
  binary feature, and `std`.
- **[`neco-plist`](https://crates.io/crates/neco-plist) 0.1.0 — rejected.** Zero
  dependencies, which is the right shape, but it is an *XML subset* parser only.
  Drag-created `.webloc` files are binary, so it covers the encoding this crate
  needs least.
- **[`openstep-plist`](https://crates.io/crates/openstep-plist) — not
  applicable.** A different plist dialect (Glyphs font sources), not the two
  encodings a `.webloc` uses.
- **`plutil` / `libplist` bindings — not applicable.** A codec crate cannot
  shell out or link a C library.

Verdict: no dependency. A reader for `bplist00` that only resolves strings and
one dictionary is small and self-contained, and it keeps the `no_std` promise
that the `plist` crate cannot.

## Strings come back as `Text`, not `&str`

Because the bytes are often not the value:

| Case | Why |
|---|---|
| XML with entity references | CoreFoundation writes `&` as `&amp;`, so *every* URL with two query parameters is escaped. Returning the raw slice would be wrong in the most common case there is |
| bplist `0x6n` string | UTF-16 big-endian, which cannot be borrowed as a `&str` at all |
| Everything else | Plain UTF-8, borrowed directly — `Text::as_str` returns `Some` |

`Text` iterates as `char`s in all three cases, compares against a `&str` with
`eq_str` without materialising anything, and grows `to_string_lossy` behind the
`alloc` feature.

## Security

- Binary plist **object references are indices into an attacker-controlled
  offset table**, and each entry in that table is a file position. Both are
  validated: the trailer's `num_objects`, `top_object` and `table_offset` are
  checked against the real file length once at parse time, and every object
  offset must land *before* the offset table — otherwise a crafted offset makes
  the reader parse the table, or the trailer, as an object.
- A reference can name its own container. Traversal is charged against
  `rclip_core::MAX_DEPTH`, and the one place `.webloc` reading can meet a cycle
  — a `URL` value that points back at the root dictionary — is rejected as a
  type error before it gets that far.
  `corpus/synthetic/rclip-webloc/bplist-self-referential.webloc` is that case.
- The XML scanner **skips the doctype without interpreting it** and resolves
  only the five predefined entities plus numeric references. A custom entity is
  an error, not an expansion, so the classic billion-laughs input is a parse
  failure rather than an out-of-memory.
- Nested dictionaries are not searched. A `URL` key one level down is not the
  document's URL, and treating it as one would let a crafted file redirect a
  reader that takes the first match.

## What is not implemented

- **`bplist15` / `bplist16`.** A different object encoding. Rejected with
  `ErrorKind::Unsupported` rather than misread under version-00 rules.
- **Everything in a plist that is not a string.** Numbers, dates, data, arrays
  and sets decode to `Object::Other`. A `.webloc` has no use for them, and each
  type left undecoded is a type that cannot be a parser bug.
- **CDATA sections and mixed content** in XML values. Encountering one is
  `ErrorKind::Unsupported`. Left alone in phase 4 on evidence rather than by
  omission: CoreFoundation's XML writer escapes with entity references and
  never emits CDATA, confirmed by handing `plutil -convert xml1` a string
  containing `&`, `<`, `>`, a quote and a literal `]]>` and getting entities
  back for all of them. Accepting CDATA would add an unexercised branch to a
  parser whose input is written by another process.
- **Writing a resource fork,** and the rest of the Resource Manager: compressed
  resources (`resCompressed`, which `rsrc` reports and does not decompress),
  the AppleDouble and AppleSingle containers a fork travels in off an HFS
  volume, and resource *editing*. Reading a `url ` resource is the whole of what
  a location file needs.
- **No serializer.** `PLAN.md` scopes writers to `shell-link`, `cf-html` and
  `dropfiles`.

## Fixtures

`corpus/synthetic/rclip-webloc/`, each with a `.json` sidecar.

| Fixture | Expect | What it pins down |
|---|---|---|
| `finder-created.webloc` | ok | **Real capture** — written by Finder on macOS 15.5 (XML) |
| `finder-binary.webloc` | ok | **Real capture** — the same, through CoreFoundation's binary writer |
| `bplist-utf16.webloc` | ok | `0x6n` UTF-16 **big-endian** strings, serialised by `plutil` |
| `xml-entities.webloc` | ok | `&amp; &lt; &gt; &quot;` and both numeric entity forms |
| `inetloc-urlname.inetloc` | ok | `URL` plus `URLName` |
| `bplist-self-referential.webloc` | error | `URL` value references the root dict → no hang |
| `bplist-offset-past-end.webloc` | error | Offset-table entry past EOF → `BadOffset` |
| `bplist-truncated.webloc` | error | Object data parsed as a trailer → `Malformed` |
| `bplist-too-short.webloc` | error | Below header + trailer → `UnexpectedEof` |
| `xml-no-url-key.webloc` | error | A plist, but not a location file → `Malformed` |
| `not-a-plist.webloc` | error | A renamed text file → `BadMagic` |
| `rsrc-named-resources.bin` | ok | A resource fork with named resources and a negative ID |
| `rsrc-no-url-resource.bin` | error | A fork with no `url ` resource → `Malformed` |
| `rsrc-map-past-end.bin` | error | Header disagrees with the buffer → `BadMagic` |
| `rsrc-type-list-past-map.bin` | error | Type list outside the map → `BadOffset` |
| `rsrc-data-offset-past-end.bin` | error | Data block outside the data area → `BadOffset` |

Plus one capture outside this directory: `corpus/macos/finder/webloc-resource-fork.bin`
is the **resource fork of the same file** whose data fork is `finder-created.bin`,
read out of its `..namedfork/rsrc` stream. `DeRez` on the original lists the same
three resources this crate parses. The synthetic resource-fork fixtures were built
with Apple's own `Rez(1)` rather than assembled by hand, so their layout is the
Resource Manager's and not this repository's reading of it.

All five plist `ok` fixtures round-trip through `plutil -p`, and `plutil` rejects
all six malformed plist ones too.

The two `BadMagic` / `BadOffset` resource-fork fixtures are a pair on purpose.
Recognising a resource fork *is* checking its header, because the format has no
magic number, so a header that disagrees with the buffer means "not this format"
— `BadMagic` — while a sound header with a broken map means "this format, broken"
— `BadOffset`. Collapsing detection into a full parse would report every
structural error inside the map as `BadMagic`, which tells a caller the wrong
thing.

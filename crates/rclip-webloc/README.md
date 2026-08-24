# rclip-webloc

Reader for macOS **`.webloc`** and **`.inetloc`** internet location files — a
property list whose single dictionary has the key `URL`, plus `URLName` on a
`.inetloc`. Reads both encodings that occur in the wild: XML plists and
`bplist00` binary plists. `#![no_std]`, `#![forbid(unsafe_code)]`, borrows from
the caller's buffer, no dependencies beyond `rclip-core`.

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

- **The legacy resource-fork form.** Pre-OS X internet location files had an
  empty data fork and carried the URL in `url ` and `drag` resources; Finder
  still writes those resources alongside the plist today, as the captured
  fixture's own resource fork shows. Reading them needs a resource-fork reader,
  which is a separate format. `// TODO(phase-4):` in `Webloc::detect`.
- **`bplist15` / `bplist16`.** A different object encoding. Rejected with
  `ErrorKind::Unsupported` rather than misread under version-00 rules.
- **Everything in a plist that is not a string.** Numbers, dates, data, arrays
  and sets decode to `Object::Other`. A `.webloc` has no use for them, and each
  type left undecoded is a type that cannot be a parser bug.
- **CDATA sections and mixed content** in XML values. Property lists do not
  contain them; encountering one is `ErrorKind::Unsupported`.
  `// TODO(phase-4):` in `src/xml.rs`.
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

All five `ok` fixtures round-trip through `plutil -p`, and `plutil` rejects all
six malformed ones too.

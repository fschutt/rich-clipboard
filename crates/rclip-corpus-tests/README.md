# rclip-corpus-tests

The whole-corpus gate. `publish = false`; it exists to be run, not consumed.

Twelve codec crates each sweep their own fixture directory. That checks the corpus twelve times
and never once as a whole, which leaves two things invisible.

**Orphans.** A `.bin` with no sidecar, a sidecar whose `.bin` was deleted, a whole directory
nobody routes — all of it passed before this crate existed, because a per-crate sweep only ever
looks in its own directory and only ever at the files it names.

**Leaks.** A captured fixture is cut from a real machine.
`corpus/synthetic/rclip-bookmark/corefoundation-file.bin` arrived carrying a boot-volume UUID and
a sandbox-extension HMAC, and it was caught by a human reading a report. That is not a control.

## What the gate checks

`tests/corpus_gate.rs`, nine tests:

| Test | What fails it |
|---|---|
| `no_orphans_in_either_direction` | a `.bin` with no `.json`, a `.json` with no `.bin`, or anything in `corpus/` that is neither |
| `every_sidecar_meets_the_contract` | a missing or wrong-typed `format` / `origin` / `description` / `expect`; a `"captured"` fixture with no `os` / `app` / `how`; an `"error"` fixture that does not name its `ErrorKind`; a mistyped key; a `leak_allow` with no reason |
| `no_fixture_is_enormous` | a fixture over 256 KB, or a sidecar over 64 KB |
| `every_directory_is_routed_to_a_parser` | a new `corpus/synthetic/<crate>/` that `exercise` does not route, or that has no `crates/<crate>/`; a capture directory that is not `<platform>/<app>/` |
| `every_fixture_produces_what_its_sidecar_promises` | an `"ok"` fixture that any accessor rejects; an `"error"` fixture that parses; an `"error"` fixture that fails with a kind other than the one it declared |
| `captures_live_where_captures_live` | hand-built bytes outside `corpus/synthetic/`, or a fifth capture filed under `synthetic/` |
| `a_recorded_byte_count_matches_the_file` | a sidecar `bytes` that disagrees with the `.bin` — a capture cut short on the way in, or a `.bin` edited afterwards |
| `the_router_is_not_a_rubber_stamp` | a routing arm that accepts 45 bytes of English prose |
| `kinds_round_trip_through_their_names` | the sidecar vocabulary drifting from `rclip_core::ErrorKind` |

Two of those are worth spelling out.

**Routing goes deeper than `parse()`.** Several fixtures are structurally fine and only fail when
a value is decoded or a list is walked: a `.desktop` value ending in a lone backslash, a `%`
escape truncated by the end of a URI line, a `.url` with no `URL` key, a bookmark whose entry
points past the end. A gate that stopped at the entry point would call all of those "declared
error but parsed cleanly". Every arm of `exercise` therefore walks as far as the format goes,
which is also what makes `expect: "ok"` worth something — an `Ok` here is a fixture every
accessor agreed on, not one that survived a header check.

**The declared kind has to be *among* the errors, not the first of them.** These formats are
deliberately built so one broken part does not cost you the rest: a `CIDA` child with a bad
`aoffset` must not poison its parent, and a bookmark TOC entry that points past the end must not
invalidate the entry before it. So a deep walk of a malformed fixture legitimately produces more
than one error, and "the error this fixture is about" is not always the first one. The gate
collects them all and asks whether the declared kind is in the list; when it is not, the failure
message prints every kind and offset that was produced.

**Captures route on `flavor`, then on `format`, never on the directory.** A directory under
`corpus/macos/` is named after an application, and an application does not name a format:
`corpus/macos/safari/` alone holds RTF, bare HTML, UTF-8 text, UTF-16 text with a BOM, two binary
plists and one WebKit-internal blob. So `route_capture` reads the sidecar's `flavor` — this
workspace's own vocabulary, `Rtf` / `Html` / `PlainText` / `Png` / `Tiff` / … — and falls back to
the `format` string for the flavours that are several formats wearing one name (`FileList` is
`CF_HDROP` *and* `text/uri-list` *and* `public.file-url`). A `CorePasteboardFlavorType 0xNNNNNNNN`
name is decoded as the four-character OSType it is, so `0x75743136` becomes `ut16` rather than a
magic number in a match arm.

Three consequences worth naming:

- **`Flavor::Html` is two payloads.** On Windows it is `CF_HTML`, with a `Version:` header; on
  macOS it is bare markup, which the CF_HTML parser correctly rejects with `BadMagic`. The router
  sniffs for the header. That sniff belongs here and not in the codec: the flavour name says
  "HTML" and does not say which spelling arrived.
- **Some formats this workspace will never decode.** A `.webarchive`, a PNG, Preview's private
  `PVPboardInfoPboardType`. Routing them to nothing would leave them sitting in the corpus
  unchecked, so each gets the strongest check that is honestly available — a binary-plist parse
  for the plist-shaped ones, a signature check for the images, non-emptiness for WebKit's
  undocumented blob — and each is listed by name with a reason, so "no codec claims this" is a
  decision somebody wrote down rather than an omission.
- **Plain text is checked as an encoding.** `public.utf8-plain-text` has to be valid UTF-8 and
  `public.utf16-external-plain-text` has to decode from its BOM, including the surrogate rules.
  That is the whole correctness question for a text flavour, and a capture claiming `ok` should
  have to answer it.

**Where the kind comes from.** `error_kind`, then `expect_error_kind`, then an `ErrorKind::Foo`
found in `notes`. Both spellings of a kind are accepted — the variant name (`BadOffset`) and the
string `ErrorKind::as_str` returns (`bad offset field`) — because `rclip-idlist` writes one,
`rclip-rtf` writes the other, and both crates' own tests depend on the spelling they chose. Only
the qualified `ErrorKind::Foo` counts in prose; a bare `Malformed` in a sentence is a sentence.

## The leak scanner

`tests/leak_scan.rs` and `src/scan.rs`. Every fixture's bytes are scanned as ASCII **and** as
UTF-16LE at both alignments — half of these formats store their strings that way, so a scanner
that only reads ASCII is a scanner that misses every Windows capture. Every sidecar's *decoded*
string values are scanned too, per key, arrays element by element: a scrubbed fixture whose `notes` quote the original value
in prose leaks it exactly as effectively as the bytes did, and it is the easier mistake to make,
because the bytes are the part you remember to scrub. Values are decoded before scanning, so a
name spelled in `\uXXXX` escapes is not a hiding place.

Six rules:

| Rule | Fires on | Allowed without comment |
|---|---|---|
| `CurrentUser` | the login name or home directory of whoever ran the scan, derived from `USER` / `LOGNAME` / `USERNAME` and `HOME` / `USERPROFILE` | a generic login (`root`, `runner`, `ubuntu`, `ci`, …) or one under three characters — matching those is all noise |
| `HomePath` | `/Users/<name>`, `/home/<name>`, `\Users\<name>` | a documented set of placeholder names: `me`, `example`, `testuser`, `foo`, `alice`, … |
| `Uuid` | `8-4-4-4-12` hex, ASCII or UTF-16LE | any uniform-digit placeholder, version and variant nibbles excepted — the nil UUID, the all-`f` sentinel, and `00000000-0000-4000-8000-000000000000`, which is what a scrubber should write when the original was a v4 — plus the OLE reserved block `…-C000-000000000046` and the published shell CLSIDs |
| `HexRun` | 32+ hex characters in a row | a run of one repeated digit — no real key or token is thirty-two identical characters |
| `Email` | `local@domain.tld` | the RFC 2606 / RFC 6761 reserved domains: `example.com`, `.example`, `.test`, `.invalid`, `localhost` |
| `PersonalDevice` | a capitalised name, a possessive, and a piece of hardware — `Someone's MacBook Pro`, `Someone's iMac`, in ASCII or with a typographic apostrophe | everything else, including `Macintosh HD` |

### Why these shapes, and not more

A check that cries wolf gets switched off, which is strictly worse than not having one. Each rule
is written around a shape that synthetic test data does not naturally take:

- **The current user is derived, never hardcoded.** A scanner that only knows one contributor's
  name is a scanner that passes for everybody else. It is also *skipped* for shared logins,
  because `runner` appearing in fixture bytes on a GitHub runner means nothing.
- **`Macintosh HD` is fine; a personalised volume name is not** — and personalised names cannot
  be enumerated. What can be expressed is the shape personalisation takes: a capitalised
  possessor, `'s`, and a hardware word. That is why the rule is `PersonalDevice` and not a list
  of names. Prose like `the user's disk` does not match (lowercase common possessor); `Apple's
  Mac` does not match (known vendor); `Macintosh HD` has no apostrophe at all. The typographic
  apostrophe U+2019 is handled, in UTF-8 and in UTF-16LE, because that is the one macOS actually
  writes.
- **A shell CLSID is not a volume UUID.** Every `{________-____-____-C000-000000000046}` is a
  published Microsoft identifier, and `.lnk` / `ITEMIDLIST` fixtures are full of them. Without
  that carve-out the shell-link corpus would be permanently red.
- **Placeholders are allowed everywhere**, so a correctly redacted capture is silent: the nil
  UUID and an all-zeros hex run are what the redaction policy tells you to substitute. The UUID
  rule is deliberately a little wider than the policy: a scrubber that keeps the version and
  variant nibbles leaves a *structurally valid* v4 UUID, which matters to anything that validates
  one and carries no more information than the nil UUID does. Thirty zeros with two nibbles set is
  never an identifier.

Failures name the file, the byte offset, the encoding, the matched text and what to do about it:

```
synthetic/<crate>/<fixture>.bin  HomePath at byte 24 (UTF-16LE): "\\Users\\<name>"
  — "<name>" is not one of the placeholder user names (me, user, username, someone, somebody,
  anon); if this is synthetic, use one of those, otherwise scrub it
```

The offset is a real offset into the real file, UTF-16 matches included, so `xxd -s 24` lands on
it.

### The escape hatch

A sidecar can excuse its fixture from a rule:

```json
"leak_allow": "HomePath",
"leak_allow_reason": "Both paths are hand-built and spell a Windows home directory under a first name …"
```

`leak_allow` is a comma-separated list of rule names (or `*`). `leak_allow_reason` is **required**
beside it — an allowlist entry with no reason is one nobody can ever safely remove — and
`allowlist_entries_name_real_rules` fails on a rule name that does not exist, because a typo
silences nothing while looking like it silences something. One fixture in the corpus uses it
today; the reason says why and what to do if the `.bin` is ever rebuilt.

`CurrentUser` is deliberately *not* excusable in practice: a path naming the running user is
reported under `CurrentUser` rather than `HomePath`, so a `HomePath` allowlist cannot hide it.

### The scanner has to be able to fail

`the_scanner_catches_a_deliberately_leaky_payload` feeds it a payload built to trip every rule, in
ASCII and again in UTF-16LE, and fails if any rule stays quiet. Without that, a rule that silently
stopped matching would leave every run green and the corpus unguarded — which looks identical to a
clean corpus.

## Why a hand-written JSON reader

`src/json.rs` is about 400 lines including tests and the sidecars are a flat object with six keys
that matter, so the alternative was `serde_json`. Against it:

- **`plan/CONVENTIONS.md` rule 7 is "minimal dependencies", and every codec in this workspace
  holds to it** — `rclip-core` and nothing else. The crate whose job is guarding the corpus is a
  poor place to be the first to break that, even in a dev-dependency, and dev-dependencies are
  still a supply chain: `serde_json` pulls `serde`, and anything reaching for `derive` pulls
  `syn`, `quote` and `proc-macro2` behind it. The conventions doc's own bar for taking a
  dependency — "more than ~3 transitive deps … is too much" — is not met.
- **The contract wants a reader that refuses things**, and `serde_json` is built to accept them.
  A nested object, an array, a duplicate key and a raw control character in a string are all
  silently fine to a general-purpose parser (a duplicate key just wins) and are all *reportable
  defects* in a sidecar. Enforcing "a flat object of scalars" is easier when it is the only thing
  the parser can do.
- **The reader has one caller and one shape.** There is no schema evolution to absorb, no
  performance question at 127 files, and no partial-input case.

What the reader does *not* skimp on is escape decoding: `\uXXXX`, surrogate pairs, and every
short escape are decoded properly, because the leak scanner reads sidecar prose and a scanner
that only saw the raw file would miss anything spelled in `\u` escapes. Lone surrogates are an
error rather than a `U+FFFD`, for the same reason they are an error in `rclip-rtf`.

The tests are the honest measure: `src/json.rs` carries seven of its own, covering the escape
cases, the refusals and the duplicate key.

## Prior art

Per `plan/CONVENTIONS.md`. The pieces here are a corpus walker, a sidecar validator and a
secret scanner.

- **`serde_json` / `json`** — see above. Not taken.
- **`gitleaks`, `trufflehog`, `detect-secrets`** — real secret scanners, and the right answer for
  a repository full of source. Wrong shape here: they are tuned for high-entropy credentials in
  text and would not look at UTF-16LE inside a binary blob, would not know that a nil UUID is a
  deliberate placeholder or that `…-C000-000000000046` is a published constant, and cannot be
  told "this fixture is allowed one match, and here is why" from the sidecar. Their false-positive
  profile on 127 binary fixtures is the failure mode this crate is trying to avoid. Not taken;
  they remain worth adding at the repository level for the *source*, which is a different job.
- **`insta` / `goldenfile`** — snapshot testing. Not the problem: the corpus already has its
  expectations written down in sidecars, and the gap was that nothing checked them.
- **`walkdir`** — a fine crate; `std::fs::read_dir` with an explicit queue is twenty lines and
  handles the one case that matters here, a directory appearing or vanishing mid-walk while
  another agent writes `corpus/macos/`.

## Running it

```sh
cargo test  -p rclip-corpus-tests
cargo clippy -p rclip-corpus-tests --all-targets -- -D warnings
cargo fmt   -p rclip-corpus-tests --check
```

CI runs the gate and the leak scan as separate jobs in `.github/workflows/corpus.yml`, so a red X
says which of the two questions failed.

## Sidecar shape

Beyond the four required keys, the reader accepts:

| Key | Type | Meaning |
|---|---|---|
| `notes` | string | prose; scanned for leaks, and mined for an `ErrorKind::Foo` |
| `error_kind` / `expect_error_kind` | string | the kind an `"error"` fixture must produce |
| `os`, `app`, `how` | string | required of a `"captured"` fixture |
| `os_version`, `app_version`, `captured_at` | string | optional provenance |
| `flavor` | string or null | the `rclip_core::Flavor` this was offered as; `null` when none fits |
| `bytes` | number | payload size, cross-checked against the file |
| `item` | number | which `NSPasteboardItem` a multi-item capture came from |
| `redacted` | bool | something was scrubbed; `notes` must say what |
| `leak_allow`, `leak_allow_reason` | string | the leak scanner's escape hatch |
| `expect_*` | any scalar, or a flat array | per-codec pinned expectations |

Anything else is a failure, because a sidecar with an `"expct"` key documents nothing and no test
would ever notice. Values are scalars or one-level arrays of scalars; a nested object is refused.

## Not implemented yet

- `MAX_FIXTURE_BYTES` is a flat 256 KB. A DIB capture will legitimately be larger than a `.url`
  file; per-format budgets would be more useful once there are captures to size them from.
  <!-- TODO(phase-2): per-directory size budgets. -->
- The scanner reads UTF-16**LE** only. `rclip-webloc` binary plists carry UTF-16**BE** strings,
  which no current fixture uses for anything identifying, but a real Safari capture could.
  <!-- TODO(phase-2): a UTF-16BE projection alongside the LE one. -->
- Nothing checks that a redaction kept the original byte length, because the original is gone by
  the time the fixture is committed. `a_redacted_fixture_says_what_was_replaced` checks only that
  the sidecar says what was done.

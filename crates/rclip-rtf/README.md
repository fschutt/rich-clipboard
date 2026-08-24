# rclip-rtf

RTF reader for clipboard-grade styled text — `no_std`, `forbid(unsafe_code)`, no allocation
while parsing.

**Format:** Rich Text Format 1.9.1 (Microsoft, "Rich Text Format (RTF) Specification, Version
1.9.1"). Semantics were checked against the published spec text rather than written from memory;
the passages that mattered are quoted in the doc comments where the rule is implemented.

**Why this one first:** on macOS `public.rtf` is *the* rich flavor — Pages, TextEdit, Mail and
Notes all speak it and several speak no HTML at all. On Windows, Word and Outlook offer it
alongside `CF_HTML` and it is the higher-fidelity of the two. See `plan/PLAN.md` §4.3.

## Shape

```text
bytes ──▶ Tokenizer ──▶ Parser ──▶ StyledRun        no_std, borrowing
                    └─▶ fonts() / colors() / generator()
                                    └──▶ Document   feature "alloc"
```

`Tokenizer` is a pure lexer with no state beyond a cursor. `Parser` adds the group stack, the
destination rules and the `\ucN` skip counter.

### The `alloc` boundary

Parsing never allocates. The one thing that genuinely cannot be done without an allocation is
handing back *decoded* text as a single string, because `\uN` and `\'hh` mean the characters of a
document are not a contiguous slice of its bytes anywhere. So `StyledRun` carries either a
borrowed `&str` (the common case) or one decoded `char`, and runs are not merged; `Document`
behind `alloc` owns one `String` plus `Run`s that are byte ranges into it, with adjacent
equal-property runs merged.

## Prior art

Checked crates.io, docs.rs and the GitHub sources. Five crates parse RTF at all, three of them
being the same codebase. **None is `no_std`** (zero `#![no_std]` declarations across all five),
and `rtf-parser` and `rtf-grimoire` each have zero reverse dependencies.

- **`rtf-parser`** (0.4.3, Jun 2026) — the most maintained option and the only one that decodes
  `\uN`. Its `\ucN` save/restore *is* per-group and correct, but the skip itself is a value
  heuristic over consecutive `\u` tokens (`if unicodes[i] <= 255 && ignore_counter < ...`) rather
  than a positional character skip, because the lexer collapses `\'hh` and `\uN` into the same
  token type. It therefore only works when the fallback is `\'hh`; the plain-`?`, group and
  control-word fallbacks that Word, WordPad and macOS actually emit all leak into the output.
  Also panics on a lone surrogate (`String::from_utf16(...).unwrap()`), takes `&str` so it cannot
  ingest real code-page RTF bytes at all, and pulls 25 crates by default including two copies of
  `syn` and `wasm-bindgen`. **Rejected.**
- **`rtf-parser-tt`** (0.5.0, Dec 2025) — a fork of `rtf-parser` that moves the wasm bindings
  behind an off-by-default feature. Byte-identical `\uc` behaviour and byte-identical failures.
  **Rejected.**
- **`rtf-to-html`** (0.1.0, Dec 2025) — thin wrapper over `rtf-parser-tt`, inherits its `\uc`
  bugs, and its GitHub repository 404s. **Rejected.**
- **`rtf-grimoire`** (0.2.1, Apr 2023) — an honest raw tokenizer with the cleanest dependency
  tree (`nom` 7 only) and the right byte-oriented input model. It implements no `\uc`, no `\uN`
  and no destination skipping whatsoever: every semantic is the caller's problem, which is the
  whole of the work here. `std`-only, allocates a `String`/`Vec<u8>` per token, materialises the
  entire token vector up front, unmaintained since 2023. **Rejected**, but its input model is the
  right one and this crate's lexer takes the same byte-oriented approach.
- **`scrivener-rtf`** (0.1.0, Mar 2026) — parses `\uc` into the AST and never acts on it, and
  never decodes `\uN` to text at all while still decoding `\'hh`, so unicode output is inverted:
  the fallback bytes survive and the real character is dropped. **Rejected.**
- **`striprtf`** — not a Rust crate (Python/Julia/R only). **`rtf`** on crates.io is an alias of
  `rtforth`, a Forth interpreter. `compressed-rtf` is MS-OXRTFCP decompression and
  `dazzle-backend-rtf` is a writer; neither parses.

**Verdict: written from scratch.** No candidate is `no_std`, none parses without allocating, and
the one thing the plan singles out as the make-or-break detail — the `\ucN` skip counter — is
either absent, ignored, or implemented as a heuristic that fails on the fallback form real
writers emit.

## What is implemented

- **Lexer.** Groups, control words (`\word`, `\word42`, `\word-42`, exactly one delimiting space
  consumed), control symbols (`\\` `\{` `\}` `\*` `\~` `\-` `\_` `\'hh`, backslash-newline), text
  runs borrowed as `&str`. CR/LF/NUL dropped as stream artefacts. `\binN` payloads consumed by
  the lexer, length-checked through `Reader`.
- **The `\ucN` skip counter**, per group, saved on `{` and restored on `}`. Counts *characters*:
  `\'hh` is one, any control word or symbol is one, `\bin` plus its payload is one, and a brace
  ends the skip early.
- **`\uN`** as a signed 16-bit value with negative wraparound, surrogate pairs joined across two
  consecutive escapes, lone surrogates replaced rather than emitted or panicked on.
- **Destinations.** `{\*\unknown ...}` dropped wholesale including nested groups; unmarked
  unknown destinations keep their text; a curated list of known non-body destinations
  (`\fonttbl`, `\colortbl`, `\pict`, `\info`, `\header`, `\pntext`, `\listtext`, ...) suppressed
  even without a `\*`. `\fldrslt`, `\shptxt` and `\result` are deliberately *not* on that list.
- **Header tables read:** `\fonttbl` (both the sub-group form and the older flat form, with
  nested `{\*\panose}` / `{\*\falt}` groups excluded from the name), `\colortbl` (omitted "auto"
  entries preserved as `None`), `{\*\generator}`.
- **Character properties:** `\b \i \ul \ulnone \ul*` `\strike` `\fsN` (half-points) `\cfN` `\cbN`
  `\highlightN` `\fN` `\plain`.
- **Paragraph:** `\par \pard \line \tab \sect \page`, plus `\cell`/`\row` as tab/break.
- **Header:** `\rtfN` `\urtfN` `\ansi \ansicpgN \mac \pc \pca \deffN`.
- **Bounds.** Nesting is capped at `rclip_core::MAX_DEPTH` with a fixed-size stack and a loop.
  There is no recursion in the crate at all, so `{{{{{...` returns `ErrorKind::DepthLimit`.

## Code pages (the `codepage` feature)

**Done** — was `// TODO(phase-1):`. Phase 0 implemented Windows-1252 and Latin-1 and decoded
everything else to U+FFFD, on the grounds that a wrong guess produces mojibake that looks like
real text and survives into the user's document while a replacement character is at least
visibly a gap. That default is unchanged.

The optional, default-off `codepage` feature adds the tables, via `rclip-codepage`:

| RTF keyword | Code page | Now decodes as |
|---|---|---|
| `\ansi`, no `\ansicpg` | 1252 (assumed) | Windows-1252 — built in, no feature needed |
| `\ansicpg819`, `\ansicpg28591` | 819 / 28591 | ISO-8859-1 — built in |
| `\ansicpgN`, N in 1250–1258 | 1250–1258 | the matching Windows page |
| `\mac` (or `\ansicpg10000`) | 10000 | Mac OS Roman |
| `\pc` | 437 | IBM CP437 |
| `\pca` | 850 | IBM CP850 |

`Codepage::Unsupported(n)` keeps its name and keeps carrying the raw `\ansicpg` number — one
variant rather than thirteen, so adding an encoding cannot break a caller's `match`, and so a
page nothing implements is still reported as the number it was. Ask `is_supported()` rather than
matching on the variant, and `encoding()` for the `rclip-codepage` handle.

Two things the feature does **not** change. A code page nothing implements (KOI8-R, say) still
decodes to `None`/U+FFFD rather than being approximated. And `is_supported()` never meant
"decodes every byte": Windows-1253 assigns no character to `0xAA`, so `decode` returns `None`
there even though the page is fully supported.

Fixtures: `hex-escapes-cp1251.bin`, `mac-charset.bin`, `pc-charset-cp437.bin`.

## Not implemented in phase 0

- **The writer.** `// TODO(phase-2):` `StyledRun[] -> Vec<u8>` behind `alloc`, emitting a minimal
  font/colour table and escaping every non-ASCII character as `\uN` with an ASCII fallback. It
  must never emit a raw high byte, because the reader on the other end may be running under a
  different `\ansicpg` than we assumed.
- **Paragraph properties.** `\pard` is accepted and resets nothing; alignment, indents and
  spacing are not modelled. `// TODO(phase-1):`
- **Tables, lists, fields and pictures** contribute no structure. Cells are separated by a tab
  and rows by a break so cell text does not run together, but there is no table model.
  `// TODO(phase-1):`
- **CJK code pages** for `\fcharset` fonts. Multi-byte and stateful, so they need a different
  shape of decoder than a 128-entry table. `// TODO(phase-2):`
- **`\upr`.** The ANSI half is read and the `{\*\ud}` Unicode half skipped — which is exactly the
  behaviour the construct was designed to give old readers, so it is correct but lossy.
  `// TODO(phase-1):` prefer the `\ud` half.
- **Underline styles.** Every `\ul*` variant collapses to a boolean.
- **The shared `RichText` hub type** of `plan/PLAN.md` §4.3 does not exist in `rclip-core` yet;
  `Document` is this crate's stand-in and should convert to it in phase 1.

## Where the spec and observed reality disagree

- The spec says `\uN`'s parameter is signed 16-bit and that values above 32767 are written
  negative. Writers that are not Word get this wrong in both directions: some emit unsigned
  0..65535, some emit the full scalar value (`\u128512` for an emoji). Both are accepted, because
  rejecting them loses real characters and gains nothing.
- The spec caps control words at 32 letters. No limit is enforced: a longer one is by definition
  not a spec control word, so it fails every lookup on its own, and rejecting the document over
  it would throw away content for no safety gain.
- The spec says the letter sequence is lowercase `a-z`. Uppercase is accepted into the *name* so
  that a stray `\Foo` lexes as one unknown control word rather than a control word plus the body
  text `oo`. Lookups stay case-sensitive, as the spec requires.
- `\*` is honoured only as the first token inside `{`. A literal asterisk in body text needs no
  escape, so a `\*` anywhere else is malformed input; treating it as a destination marker there
  would silently drop the rest of the group.
- `CF_RTF` arrives off the Windows clipboard NUL-terminated, which the spec does not mention. The
  NUL is dropped as a stream artefact, like CR and LF.
- `{\*\generator}` is emitted by Word (`Riched20 10.0.19041;`) but **not** by Cocoa — verified
  against `textutil` output, which writes `\cocoartf2822` instead and no generator destination at
  all. Do not rely on it being present.
- Cocoa writes the *flat* `\fonttbl` form (`{\fonttbl\f0\froman\fcharset0 Times-Roman;\f1...}`)
  with no per-font sub-groups, so both forms are supported. Verified against real `textutil`
  output, which also emits `{\*\expandedcolortbl;;\cssrgb\c0\c0\c0;}` — a `\*`-marked
  destination whose semicolons would corrupt a colour table scanner that matched on a prefix.

## Fixtures

`corpus/synthetic/rclip-rtf/`, twelve documents with `.json` sidecars. They use the `.rtf`
extension rather than the `.bin` of `plan/CONVENTIONS.md` because they are real RTF documents and
open in TextEdit, which is useful when one of them starts disagreeing with a real writer.

Four are malformed and assert an `ErrorKind` rather than a panic: `unclosed-group.rtf`
(`UnexpectedEof`), `extra-close-brace.rtf` (`Malformed`), `depth-bomb.rtf` (`DepthLimit`, 128
nested groups) and `not-rtf.rtf` (`BadMagic`, HTML offered under an RTF flavor). Beyond those,
the test suite feeds every prefix of every fixture, and 400 three-byte mutations of each, through
every entry point and asserts only that nothing panics.

## Verify

```sh
cargo test  -p rclip-rtf
cargo build -p rclip-rtf --no-default-features
cargo clippy -p rclip-rtf --all-targets --all-features -- -D warnings
```

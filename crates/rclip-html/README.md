# rclip-html

Minimal HTML tokenizer for clipboard payloads — markup in, styled text runs out. `no_std`,
`forbid(unsafe_code)`, borrowing, no dependency but `rclip-core`.

**Format:** the WHATWG HTML Living Standard, in the narrow sense that its tokenizer states and its
named-character-reference table are what this implements a subset of. **This is not a browser** —
see the scope boundary below, which is the most important section in this file.

**Why it exists:** `rclip-cf-html` reads the Windows `CF_HTML` header and hands back the fragment
as markup, and says outright that it does not parse it. Until this crate, nothing else in the
workspace did either, so an HTML clipboard flavor decoded to a string of tags rather than to
styled runs — the one missing leg of `rich-clipboard`'s `RichText` hub. See `plan/PLAN.md` §4.3.

## Shape

```text
bytes ──▶ Tokenizer ──▶ Runs ──▶ Run + Style        no_std, borrowing
                   └─▶ css::declarations
                               └──▶ Document        feature "alloc"
```

`Tokenizer` is a lexer with a cursor and one flag (raw-text mode). `Runs` adds the element stack,
style inheritance and the line-break rules.

### The `alloc` boundary

Tokenizing never allocates. What cannot be done without an allocation is handing back the
*decoded* text as one string: `&amp;` is one character written as five bytes and a run of
indentation is one space, so the characters of a fragment are not a contiguous slice of its bytes
anywhere. So `HtmlText` is a lazy view with an `as_str()` fast path and a `chars()` that always
works, and `Document` behind `alloc` owns one `String` plus runs that are byte ranges into it,
with adjacent equal-style runs merged.

## Scope: what this is not

Refusing to grow into a browser is the whole design. Concretely, none of the following is here and
none of it is planned:

- **No DOM.** There is no tree, no parent pointers, no node type. There is a fixed-size stack of
  open elements and a style that is copied down it.
- **No cascade, no selectors, no `<style>` rules.** A `<style>` block's text is dropped, not
  applied. A fragment that styles its text through a class arrives unstyled. This is the one real
  hole, and it is small in practice because browsers *inline* the computed styles onto the
  elements when they serialize a clipboard fragment — precisely so that the receiving application
  does not need a cascade.
- **No insertion modes, no foreign content, no fragment-parsing algorithm.** No `<svg>` or
  `<math>` namespace handling, no `<table>` foster parenting, no implicit `<tbody>`.
- **No layout, no scripting, no `document.write`, no `<base>` resolution, no URL handling at all.**
- **No encoding detection.** UTF-8 in, and a `<meta charset>` is not read. The flavors this serves
  are defined as UTF-8; the one that is not — `text/html`, where producers write UTF-16 with and
  without a BOM — is sniffed by the caller before the bytes get here.

What *is* here is the list in "What is implemented", and it is short on purpose.

## Prior art

Checked crates.io, docs.rs and the sources, with `cargo tree` run on a throwaway project for each.
The workspace rule (`plan/CONVENTIONS.md`) is: more than ~3 transitive dependencies, or anything
pulling `syn`/`serde_derive`/`regex`, is too much for a codec this small; `std`-only is
disqualifying; and the crate has to get the hard part right, which here is malformed nesting and
character references without allocating a tree.

**Nothing on crates.io is simultaneously `no_std`, small-dependency, entity-complete and tolerant
of malformed nesting without building a DOM.** The two closest are named first.

- **`html5tokenizer`** (0.5.2, Sep 2023) — the best-behaving candidate and a real WHATWG tokenizer
  rather than a browser engine: zero dependencies, no tree construction, and it ships the full
  ~2200-entry WHATWG named-character-reference table. Fed `<b><i>text</b></i>` it emits a flat
  token stream with no complaint, which is correct — nesting is the tree builder's problem. Fed an
  unknown `&fakeentity;` it raises a *recoverable* parse error and carries on, per the
  ambiguous-ampersand rule. **Rejected on `std`**: it uses `std::io`, `std::collections` and
  `str::from_utf8_unchecked`, with no feature to turn any of it off. It also owns everything —
  `StartTag.name: String`, one `Char(char)` event per character — rather than borrowing spans, so
  even ignoring `no_std` it would have needed rewriting rather than wrapping.
- **`htmlparser`** (0.2.1, Nov 2024) — the closest *structural* fit: `#![no_std]` and
  `#![forbid(unsafe_code)]` declared unconditionally, zero dependencies, ~1400 lines. It already
  tolerates exactly the malformed nesting we need (`<root><child></root></child>` parses without
  error, by its own documentation and by test). **Rejected on syntax and coverage**: it is a fork
  of `xmlparser`, so attribute values *must* be quoted — `<div class=foo>` is a hard
  `InvalidAttribute` error rather than a graceful degrade — and it never decodes character
  references in text at all, so the entity table, which is most of the work, would still have been
  ours. Between those two it saves very little.
- **`quick-xml`** (0.42.0, Aug 2026) — the leanest dependency tree of anything surveyed: two
  crates total (itself and `memchr`), and `#![forbid(unsafe_code)]`. With `check_end_names = false`
  it becomes a fully tolerant flat token stream, and 200 000 levels of nesting stream through
  iteratively without touching the stack. **Rejected on `std`** — it has no `no_std` mode at all —
  and secondarily because its MSRV is 1.86, one above this workspace's 1.85, and because character
  references come back as undecoded `GeneralRef` events with only the five XML entities available
  to resolve them.
- **`html5ever` / `markup5ever`** (0.39.0, Mar 2026) — the reference implementation, and it does
  expose a `Tokenizer`/`TokenSink` separate from tree construction. **Rejected on dependencies**:
  32 packages locked, including `phf_codegen` and `string_cache_codegen` as build dependencies,
  which pull `proc-macro2` and `quote`. Not `no_std`.
- **`lol_html`** (3.0.1, Jul 2026) — a streaming rewriter, so it genuinely does not build a DOM.
  **Rejected on dependencies**: 44 packages, including `cssparser`, `selectors`, `derive_more` and
  `thiserror`, which between them pull **two different major versions of `syn`**. Not `no_std`.
- **`tl`** (0.7.8, Jan 2024) — zero dependencies and borrows the input lifetime, and it does
  browser-style implicit-close repair correctly (`<b><i>text</b></i>` comes back as
  `<b><i>text</i></b>`). **Rejected**: it builds a full `Vec`-backed node arena, which is a DOM;
  it decodes no character references anywhere in the crate (`&amp;` survives into `inner_text()`);
  it uses raw pointers and `unsafe impl Send/Sync` for its self-referential guard type; and nothing
  in it bounds nesting depth.
- **`html_parser`** (0.7.0, May 2023) — Pest-based; 21 packages including `syn`, `serde_derive`
  and `serde_json`. Hits both banned dependencies at once. **Rejected.**
- **`scraper`** (0.27.0) and **`select`** (0.6.1) — 51 packages each, both on top of `html5ever`
  plus a selector engine; `select` still pins `html5ever` 0.26 and pulls `syn` 1 and `rand`.
  Neither is a tokenizer. **Rejected.**
- **`simple-html-parser`** (1.0.0, Nov 2024) — zero dependencies, but it copies the whole input
  into an owned `String`, owns every AST node, and is genuine recursive descent with **no depth
  bound**: 300 000 levels of `<a>` nesting aborts the process with a stack overflow, confirmed by
  test. That is precisely the failure mode this workspace's depth rule exists to prevent.
  **Rejected.**
- **`htmlstream`** (0.1.3) — zero dependencies and `&str`-slicing iterators, but a 38-line
  `lib.rs`, no entity decoding, no `no_std` declaration, and unmaintained since 2017. **Rejected.**
- **`ego-tree`** (0.11.0) — the arena `scraper`/`select` build on. Irrelevant on its own: this
  crate must not allocate a tree at all.

**Verdict: written from scratch.** The two crates that get the tokenizing right are both
`std`-only with no way out, the one that is `no_std` refuses unquoted attributes and decodes no
entities, and everything else is either a DOM builder or a dependency tree several times the size
of the thing it would be parsing.

## What is implemented

- **Lexer.** Start tags, end tags, text, comments, doctypes and bogus comments (`<?...>`),
  `<![CDATA[...]]>`. Attribute values double-quoted, single-quoted and unquoted, plus boolean
  attributes. A `>` inside a quoted value does not end the tag. A `<` that begins nothing is text,
  so `a < b` is prose. End of input ends whatever was being read rather than failing.
- **Raw-text elements.** `script`, `style`, `title`, `textarea`, `xmp`, `iframe`, `noembed`,
  `noframes`, `noscript` — everything up to the matching end tag is one text run and a `<` inside
  it is not markup. Without this the `<style>` block that every browser puts at the top of a
  clipboard fragment lexes as a stream of tags and its selectors land in the user's document.
- **Character references.** 284 named ones — the HTML 4.01 set plus the HTML5 additions that turn
  up in real markup, sorted and binary-searched — and `&#NN;` / `&#xNN;`. A missing semicolon
  still resolves, longest-match-first, so `&notin` is `&notin;` and not `&not;` followed by `in`.
  `0x80..=0x9F` decode as **Windows-1252**, not as C1 controls: `&#150;` means an en dash, HTML5
  writes the mis-mapping into the spec, and a parser that decodes it to U+0096 puts an invisible
  control character where the document meant punctuation.
- **Formatting elements.** `b`, `strong`, `i`, `em`, `cite`, `var`, `dfn`, `address`, `u`, `ins`,
  `s`, `strike`, `del`, `span`, `font`. `h1`–`h6` and `th` are bold, which is not an invention —
  it is in the UA stylesheet of every browser that ever put a fragment on a clipboard. The *size*
  increase headings also get is deliberately not applied; that one would be inventing a number.
- **Presentational attributes.** `<font face/color/size>` including the relative `+N` / `-N` form,
  and `bgcolor`. Pre-CSS mail still emits these.
- **`style=` attributes.** `font-weight`, `font-style`, `text-decoration`(`-line`), `color`,
  `background-color`, `background`, `font-size`, `font-family`. Colours as `#rgb`, `#rrggbb`,
  `#rgba`, `#rrggbbaa`, `rgb()`, `rgba()`, the modern `rgb(r g b / a)` form, and 38 keywords.
  Sizes in `pt`, `px`, `pc`, `in`, `cm`, `mm`, `q`, `em`, `rem`, `ex`, `ch`, `%` and the seven
  absolute-size keywords, all resolved to points against the enclosing element's size.
- **Breaks.** Block elements break lines, `<br>` breaks a line, `<td>`/`<th>` are separated by a
  tab and `<tr>` by a break. Breaks are emitted *lazily* — a block boundary sets a flag and the
  flag becomes a break only when text follows — which is one mechanism that removes leading
  breaks, collapses `</p><p>` into one, and drops trailing breaks, none of which a browser shows.
- **Whitespace.** Collapsed by default, preserved in `<pre>`, `<textarea>`, `<xmp>` and
  `<plaintext>`, with CR and CRLF normalized to LF and the newline immediately after a `<pre>`
  dropped. Without collapsing, a fragment copied out of a browser pastes with a newline and four
  spaces between every two words, because the serializer pretty-prints it.
- **Bounds.** Nesting is capped at `rclip_core::MAX_DEPTH` with a fixed-size stack and a loop.
  There is no recursion in the crate at all, so `<div><div><div>...` returns
  `ErrorKind::DepthLimit` rather than overflowing the stack.

## Malformed nesting is the normal case

Not an edge case — `<b><i></b></i>` is what contenteditable, mail clients and Word's HTML export
produce all day. Three rules, and all three are what a browser does:

1. **An end tag closes the nearest matching open element, or nothing at all.** A `</b>` with no
   `<b>` open closes nothing. Closing the innermost element instead — which is the tempting
   shortcut — turns one stray end tag into a document where every subsequent style is off by one.
2. **Formatting elements are reconstructed.** A `</b>` that closes a `<b>` with an `<i>` still
   open inside it reopens the `<i>`, so `<b><i>x</b>y</i>` renders `y` italic. This is HTML5's
   adoption agency algorithm reduced to its one visible consequence: the character-formatting
   elements come back, `<span>` and `<div>` do not, and the reopened styles are recomputed against
   the new parent rather than carried over.
3. **Some start tags close their predecessor.** `<p>a<p>b` is two paragraphs, and the same goes
   for `<li>`, `<td>` and `<tr>`. Without this, a document that omits its end tags nests one level
   deeper per item and hits the depth limit on its 64th list item.

Neither the search nor the reconstruction can loop: both are a single walk over a fixed-size
array, and every tokenizer step is asserted to consume at least one byte.

## Errors

There is exactly one: `ErrorKind::DepthLimit`. Everything else is absorbed — mismatched nesting,
unterminated tags and attributes, a `<` that begins nothing, invalid UTF-8 (replaced with U+FFFD),
an unknown entity (left literal). A clipboard payload is written by another process and parsed the
moment the user presses Ctrl+V; a paste that drops one character beats a paste that does nothing.

## What cannot be represented

`Style` carries bold, italic, underline, strikethrough, size, font family, foreground and
background, and that is the whole of it. So none of the following survives, whatever the markup
says: hyperlinks (`<a href>`), images, lists as lists, tables as tables, superscript and subscript,
underline *style* (dotted, wavy, double), text alignment, indents, margins, letter- and
line-spacing, `text-transform`, `opacity`, and anything a stylesheet rather than a `style=`
attribute is responsible for. That ceiling is the workspace's shared `RichText` hub, not this
crate's: see `rich-clipboard`'s `rich_text` module docs for why it is a documented boundary rather
than a phase-3 shortcut.

## Fixtures

`corpus/synthetic/rclip-html/`, ten fragments with `.json` sidecars, including a realistic browser
fragment with a `<style>` block and pretty-printed indentation, mismatched nesting with a stray end
tag, the entity edge cases, the legacy `<font>` element, all three attribute quoting forms, and a
fragment truncated in the middle of an attribute value.

`depth-bomb.bin` is the malformed one and asserts `DepthLimit` rather than a panic. Beyond it, the
test suite feeds every prefix of every fixture and several thousand single-byte mutations through
every entry point and asserts only that nothing panics and nothing loops.

## Verify

```sh
cargo test  -p rclip-html --all-features
cargo build -p rclip-html --no-default-features --target thumbv7em-none-eabi
cargo build -p rclip-html --no-default-features --features alloc --target thumbv7em-none-eabi
cargo clippy -p rclip-html --all-targets --all-features -- -D warnings
```

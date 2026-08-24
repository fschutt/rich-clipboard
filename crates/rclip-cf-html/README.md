# rclip-cf-html

`CF_HTML` — the Windows **"HTML Format"** registered clipboard format — read and written.
A payload is an ASCII description header of `Key:Value` lines followed by a UTF-8 body; the
header carries byte offsets into the whole blob that delimit the *context* (a complete
document), the *fragment* (what the user copied) and the *selection* (exactly what was
highlighted). Spec:
[HTML Clipboard Format](https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format),
with the older
[Internet Explorer page](https://learn.microsoft.com/en-us/previous-versions/windows/internet-explorer/ie-developer/platform-apis/aa767917(v=vs.85))
as the only documentation of the `SourceURL` header.

Parsing borrows: `parse(&[u8]) -> CfHtml<'_>` returns `&str` views into the caller's buffer and
allocates nothing, so it works under `#![no_std]` with no `alloc`. The serializer,
`CfHtmlBuilder`, has to own its output and lives behind the `alloc` feature.

```rust
let blob = rclip_cf_html::CfHtmlBuilder::new("<b>hi</b>")
    .source_url("https://example.com/page")
    .build()?;
let back = rclip_cf_html::parse(&blob)?;
assert_eq!(back.fragment, "<b>hi</b>");
```

## What makes this format harder than it looks

- **The offsets are absolute**, counted from the first byte of the blob — the header included.
  `StartHTML` is therefore normally equal to the header's own length.
- **They may be left-padded with any number of zeros.** That is not cosmetic: it is *how* a
  producer writes them. It reserves a fixed-width field of zeros, streams the document, then
  overwrites the digits in place. `0000000000` is the number zero and must parse as such —
  stripping leading zeros and parsing what remains turns it into an empty-string parse error,
  which is a live bug in shipped code.
- **The numbers and the `<!--StartFragment-->` comments disagree in the wild.** This crate
  believes the comments: they are markup the producer physically inserted into the same text it
  transformed, so they travel with the content, whereas the byte counts are computed separately
  and drift. Both are exposed — `Parsed::header` holds the raw claims and
  `Parsed::fragment_source` says which source won — so a caller can see the disagreement rather
  than having it silently resolved.
- **Microsoft's own worked example is one of the broken ones.** The `Scenario 1` blob on the
  current docs page has `StartFragment:0006` / `EndFragment:0106` where the comments sit at 147
  and 247. It is checked in verbatim as `corpus/synthetic/rclip-cf-html/mshtml-scenario1.bin`
  and is the fixture that pins this behaviour.
- **`StartHTML`/`EndHTML` may be `-1`**, meaning fragment-only with no context. Run through an
  unsigned parse it either errors or wraps to a colossal offset.
- **`StartSelection`/`EndSelection` are both-or-neither.** Half a pair is rejected; inventing
  the missing end silently mislabels what the user highlighted.
- **Line endings may be `\r\n`, `\n`, or a lone `\r`.** `str::lines` does not split on a bare
  CR, so anything built on it reads the whole header as a single line.
- **Unknown keys and unknown versions are skipped, not rejected.** The spec reserves the right
  to extend the header and Internet Explorer already did, with `SourceURL`.

### Writing: fixed-width placeholders, patched once

The offsets in the header are positions in the buffer the header is part of, so writing one is
self-referential. `CfHtmlBuilder` does what the spec suggests: emit `0000000000` for every
offset, remember where each field starts, append the body, then overwrite the ten digits in
place. The field width never changes, so nothing shifts and there is no second pass. Iterating
toward a fixed point — write the offsets, notice the header grew, write them again — is the
mistake this design exists to avoid; `the_header_is_the_same_size_whatever_the_payload` is the
test that pins it, comparing the header of a 1-byte fragment against that of a 200 KB one.

`-1` is the one value not written as a placeholder: a no-context blob knows it up front and it
does not fit the zero-padded shape, so those two lines are emitted literally.

## Prior art

Searched crates.io and docs.rs for a CF_HTML codec. **There is no crate that parses CF_HTML as
its own concern** — the names `cf-html`, `cfhtml`, `html-clipboard`, `clipboard-html`,
`cf_html` and `html_clipboard` are all unregistered. What exists is CF_HTML handling buried
inside OS-clipboard crates:

| Crate | Verdict |
|---|---|
| [`ironrdp-cliprdr-format`](https://crates.io/crates/ironrdp-cliprdr-format) 0.2.0 | Closest thing to a real codec, and the source everyone else copied. Rejected: pulls `ironrdp-core` with `features = ["std"]` plus `png` — 11 transitive crates (`flate2`, `miniz_oxide`, `crc32fast`, `fdeflate`, …) for a text header, and it is not `no_std`. Correctness-wise its parser reads only `StartFragment`/`EndFragment`, ignores the marker comments entirely, has no `-1` handling, rejects a fragment that ends at the last byte (`end < input.len()`), and its `value.trim_start_matches('0').parse()` turns the all-zeros field into a parse error. Its serializer *does* use the fixed-width back-patch, which is the one thing to copy — as an idea, not as a dependency. Edition 2024 / rust 1.89 also exceeds this workspace's 1.85. |
| [`clipboard-rs`](https://crates.io/crates/clipboard-rs) 0.3.5 | Vendored the IronRDP writer verbatim; its reader (`extract_html_from_clipboard_data`) returns the *context* rather than the fragment, and on `StartHTML:-1` falls through to returning the entire blob, header text and all. Windows-only, `std`, `unsafe`. Its own changelog records a bug from trusting a producer's `StartFragment` — exactly the failure this crate's comments-win rule prevents. |
| [`arboard`](https://crates.io/crates/arboard) 3.6.1 | 41M downloads, but `wrap_html` is write-only — there is no CF_HTML reader at all. Windows-only, `std`, tied to `clipboard-win`. |
| [`clipboard-win-html`](https://crates.io/crates/clipboard-win-html) 0.2.0 | The only crate on crates.io whose entire purpose is CF_HTML. Write-only, depends on `windows` 0.54, and the offsets are hardcoded constants (`start_html = 391`) that are each off by one against the header it actually emits; `end_fragment = start + len - 1` is off by one again; it writes `<!-- StartFragment -->` with the spaces the spec forbids, appends a NUL to the payload, and over-allocates by 2×. Rejected as incorrect, not merely unsuitable. |
| [`lamco-clipboard-core`](https://crates.io/crates/lamco-clipboard-core) 0.6.1 | Format *mapping* and transfer plumbing, not a CF_HTML codec. Out of scope. |

**Verdict: wrote our own.** Nothing published parses CF_HTML with the comments-over-offsets
rule, `-1` contexts, zero-padded fields or lone-CR line endings, and nothing published is
`no_std` or dependency-free. This crate depends on `rclip-core` and nothing else.

## Where the spec and reality disagree

Recorded here because each one is a decision this crate had to make:

1. **The spec's own example is wrong.** `Scenario 1`'s `StartFragment`/`EndFragment` do not
   match its own `<!--StartFragment-->` comments. Its `StartHTML`, `EndHTML`, `StartSelection`
   and `EndSelection` are all correct, which is what makes it such a good fixture.
2. **The marker comments are specified three ways on one page.** The prose demands
   `<!--StartFragment-->` "verbatim, with no whitespace chars within each comment itself"; the
   BNF grammar three paragraphs earlier writes `"<!--StartFragment -->"`; the worked scenarios
   write `<!-- StartFragment-->`. The parser accepts whitespace anywhere inside the comment.
   The serializer emits only the strict form.
3. **`SourceURL` is not in the current grammar** but is emitted by Internet Explorer, Edge,
   Chrome and Word, and is the only way to resolve relative URLs in a pasted fragment. It is
   documented only on the archived IE page. Parsed as a first-class field.
4. **`Version` has no gate behind it.** 20H2 bumped `0.9` to `1.0` with no format change, and
   plenty of current software still writes `0.9`. `CfHtmlBuilder` defaults to `0.9` for that
   reason: every consumer in existence accepts it.

## Two decisions the spec leaves open

**A leading UTF-8 BOM is skipped for line reading and *counted* for every offset.**
`EF BB BF` before `Version:` used to end the header before it began — the first line's "key" is
not ASCII, so the scan stopped and the blob came back `BadMagic`. It is now handled, and the
question the spec does not answer is which of the two readings the producer meant, since a
`CF_HTML` offset is a byte count from the start of the blob. The answer taken here: the blob
starts where the blob starts. A producer that prepends a BOM is measuring positions in the very
buffer it is writing, and that buffer has the BOM in it, so `header_len` includes the mark and
every offset resolves against the payload untouched.

A producer that disagreed is not left in ruins: its `StartHTML` comes out three bytes short and
is clamped up to the end of the header, and its fragment offsets lose to the
`<!--StartFragment-->` comments, which move with the content. The one thing it costs is the last
three bytes of the *context*, which `EndHTML` then under-reports. `Header::bom_len` is exposed so
a caller that wants to detect and compensate can compare it against `header_len`.

**No real capture carries one.** `corpus/macos/safari/public.html` and
`corpus/macos/textedit/` were checked: macOS publishes bare HTML with no `CF_HTML` description
header at all, and nothing in the corpus prepends `EF BB BF` to anything but a UTF-16 payload,
where `FF FE` is a different mark entirely. The handling is therefore defensive rather than
observed — but the old behaviour was to reject the payload outright, which is not a defensible
place to leave it.

**A repeated keyword takes its first value**, and `Header::duplicate_keys` records that it
happened. The spec floats "multiple StartFragment and EndFragment pairs [...] to support
noncontiguous selection of fragments" as a future extension. Checked against reality: nothing
emits them — not Chrome, not Firefox, not Word, not Windows itself — and there is nowhere in a
borrowed, non-allocating return type to put a list of them if they did. So a repeat today is a
producer bug or a deliberate ambiguity, and the two readings of it disagree about *which bytes
the user copied*, which makes it a parser-differential rather than a cosmetic question.
First-wins is the reading that cannot be changed by appending to a header. Rejecting the payload
was the alternative and was refused for the same reason `Version:1.1` is accepted: a paste that
silently does nothing is worse than one that drops a field nobody writes.

## Not implemented yet

- `// TODO(phase-2):` Converting the fragment into the shared `RichText` type. That belongs
  with the HTML tokenizer, not here — this crate's job ends at handing back a `&str` of markup.
- No HTML parsing of any kind. The fragment is returned as bytes-that-are-a-`&str`; it is not
  validated as markup, and the spec's "a valid fragment is a single outer element" rule is not
  enforced.

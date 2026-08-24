# rclip-desktop-entry

Parser for freedesktop `.desktop` entries — the Linux shortcut.

Spec: [Desktop Entry Specification v1.5](https://specifications.freedesktop.org/desktop-entry/latest/).
Section numbers throughout the source refer to it.

`Type=Link` plus `URL=` is the direct analogue of a Windows `.url` and a macOS `.webloc`, which is
why this crate is in a clipboard workspace: dropping a launcher on an application should yield
structure, not a filename.

`#![no_std]`, `#![forbid(unsafe_code)]`, parsing borrows from the caller's buffer and allocates
nothing. Only `Value::to_unescaped` / `to_unescaped_lossy` need the `alloc` feature; escapes decode
through a `char` iterator otherwise.

## Nothing here runs anything

`Exec=` is parsed into `exec::ExecCommand` — arguments and field codes as data. No `$PATH` lookup,
no field-code expansion, no process, no filesystem access. A `.desktop` file arriving over the
clipboard is written by another process; `plan/CONVENTIONS.md` rule 6 names this format
specifically.

## The three hard parts

**Escapes and `\;` in list values** (`src/value.rs`). §4 defines `\s \n \t \r \\`, and separately
says a semicolon inside a multi-valued key is escaped `\;`. The trap is the interaction:
**split first, unescape second.** Unescaping first turns `\;` into a bare `;`, and the split then
treats it as the separator the escape existed to suppress. `ListItems` scans for separators over the
*raw* text and hands each piece back still escaped. §4's trailing-separator rule is honoured too:
`a;b;` is two items, `a;b;;` is three.

**Localized keys** (`src/locale.rs`). §5's full four-rung ladder —
`lang_COUNTRY@MODIFIER` → `lang_COUNTRY` → `lang@MODIFIER` → `lang` → unpostfixed — with the two
consequences the spec spells out: candidates derive from the *requested* locale only (asking for
`sr` must not return `Name[sr@Latn]`), and `.ENCODING` is stripped from both sides so `de_DE.UTF-8`
matches `Name[de_DE]`. The spec's own worked example (`sr_YU@Latn` selects `Name[sr_YU]`) is a test.

**`Exec=`'s two stacked escape layers** (`src/exec.rs`). §7 is explicit that the value-level escape
rule "is applied before the quoting rule", which is why a literal backslash inside a quoted argument
is four backslashes in the file and a literal `$` is `\\$`. The scanner decodes value escapes
*while* tracking quotes rather than in a separate pass, so both of the spec's worked examples come
out right. `ExecCommand::validate` checks the rules that span the whole command line: at most one of
`%f %u %F %U`, `%F`/`%U`/`%i` only as an argument on their own, and no field code inside a quoted
argument — each of which is a way for a crafted file to hand a launcher more arguments than it
expects.

## Prior art

Three crates exist. All three were read, and each fails on something specific.

- **`freedesktop_entry_parser` 2.0.1** — cleanest dependency tree of the three (5 transitive, `nom` +
  `memchr` + `indexmap`) and the only one with `forbid(unsafe_code)`, but it treats a backslash
  *anywhere* in a value as a line continuation (`low_level.rs`: `take_till(|c| c == b'\n' || c ==
  b'\\')` then jump to the next line). `Name=Innocent\` therefore absorbs the following `Exec=` line
  into the `Name` value and the `Exec` key vanishes from the entry. Escape sequences are not
  implemented at all, by design. Not usable.
- **`freedesktop-file-parser` 0.3.1** — the textbook `\;` bug: `parser.rs` does
  `parts.value.split(";").map(|s| s.to_string())` with no unescaping anywhere, so
  `Categories=Network;Web\;Browser;Utility;` parses as four items with a dangling `Web\`. 22
  transitive dependencies including two `thiserror` majors, hence two `syn` majors, plus `tracing`
  and `freedesktop-icons`. Not usable.
- **`freedesktop-desktop-entry` 0.8.1** — best maintained (pop-os, used by COSMIC) and the only one
  that implements `\s \n \t \r \\` correctly, but `\;` is not in its match arms, so `format_value`
  hard-errors `InvalidValue` on any spec-legal file containing an escaped semicolon; the whole file
  fails to parse. Where it does parse it unescapes at parse time and splits on `;` afterwards — the
  wrong order — and it emits a spurious empty item for the spec's optional trailing separator. Its
  locale lookup implements two of the four rungs (exact, then truncate at the first `_`), and never
  strips `.UTF-8`. Its `Exec` handling is structural and does not execute — the one thing it gets
  right for our purposes — but quoting is `split_ascii_whitespace()` with a `// todo: handle
  escaping`, so `Exec="/opt/my app/bin" --flag %U` returns `Err(WrongFormat("unmatched quote"))`.
  Not usable.

All three are additionally `std`-only, allocate an owned `String` per value, and do `std::env` /
filesystem work in the same crate. **Wrote our own**: `no_std`, borrowed values, correct `\;`
ordering, the full §5 ladder, and both §7 escape layers.

## Where the spec and observed reality disagree

- §3.1 says comments are "lines beginning with a `#`" and §3.2 gives group headers no leading
  whitespace. GLib's `g_key_file_parse_line` skips leading whitespace before deciding what a line
  is, and GLib is what every `.desktop` file in the wild has actually been tested against, so
  leading whitespace is tolerated here too.
- §3 requires UTF-8 and says nothing about a byte-order mark; editors add one, and
  `\u{FEFF}[Desktop Entry]` does not start with `[`, so a BOM is stripped. Likewise §3 says lines
  are separated by linefeeds, but CRLF files exist and a `\r` left on a group header makes the name
  match nothing.
- §4 permits `\;` only inside a multi-valued key and GLib rejects it elsewhere. It is unambiguous
  everywhere, so it is accepted in every value — rejecting `Comment=either\;or` loses the comment
  and gains nothing.

## The shared shortcut type

A `Type=Link` entry's destination comes back as `ShortcutTarget<'a>`, re-exported from
`rclip_core::shortcut` and shared with `rclip-url-file`, `rclip-webloc`, `rclip-shell-link` and
`rclip-uri-list` — see `plan/PLAN.md` §4.10.

## Not implemented yet

- **Duplicate detection.** §3.2 forbids duplicate group names and §3.3 duplicate keys within a
  group. Detecting either is quadratic, and this parser's input arrives from another process, so a
  payload with a hundred thousand keys would turn the check into a hang. Lookups return the *first*
  match, which is what GLib does; duplicates are not diagnosed. Deliberate, not an oversight.
- No serializer. §3 asks a compliant implementation that rewrites a file to preserve fields it does
  not understand, which is a whole design of its own; nothing in the plan needs it before Phase 4.
- `%i` expansion, `TryExec` resolution, `OnlyShowIn`/`NotShowIn` evaluation against
  `$XDG_CURRENT_DESKTOP`, and D-Bus activation are all policy over environment, not byte format.
  They belong above this layer.

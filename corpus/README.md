# Corpus

Test fixtures for the codec crates. Every `.bin` has a `.json` sidecar describing what it is and
whether it is expected to parse.

```
corpus/
├── synthetic/<crate>/       hand-built bytes, written alongside the codec
└── <platform>/<app>/        real captures from real applications
```

## Why both

These formats are defined in practice by what applications actually emit, which regularly differs
from what the specs say. Synthetic fixtures prove the parser matches the spec; captured fixtures
prove it matches reality. A codec needs both, and where they disagree the disagreement itself is
worth recording — that is what the sidecar `notes` field is for.

## Sidecar

```json
{
  "format": "CF_HDROP",
  "origin": "synthetic",
  "description": "Two absolute paths, fWide=1, drop point (0,0)",
  "expect": "ok",
  "notes": "Hand-built to the DROPFILES layout in MS docs"
}
```

- `origin` — `"synthetic"` for hand-built bytes, `"captured"` for a real capture. Captured
  fixtures add `os`, `app` and `app_version`, plus a `how` field saying what was copied and from
  where, so the capture can be repeated.
- `expect` — `"ok"` or `"error"`. An `"error"` fixture names the `ErrorKind` it must produce in
  `notes`.

## Malformed fixtures are the point

Every crate carries at least one deliberately broken fixture: a truncated header, a length field
pointing past the end, a count that would exhaust memory, an offset that loops back on itself.
Clipboard payloads are written by another process and parsed the moment a user presses Ctrl+V, so
"returns the right error" is as much a correctness requirement as "parses the good case", and it
is the requirement a fuzzer will hammer.

Keep fixtures small. These are unit-test inputs, not sample documents.

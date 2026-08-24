# Codec conventions

Binding rules for every crate under `crates/`. Deviations need a comment saying why.

## Crate shape

```
crates/rclip-<name>/
├── Cargo.toml
├── src/lib.rs          #![no_std] #![forbid(unsafe_code)]
├── tests/<name>.rs     integration tests, loading fixtures from ../../../corpus
└── README.md           one paragraph: what format, which spec, what's unimplemented
```

Manifest inherits from the workspace:

```toml
[package]
name        = "rclip-<name>"
description = "<one line>"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[features]
default = []
alloc = ["rclip-core/alloc"]
std = ["alloc", "rclip-core/std"]

[dependencies]
rclip-core = { workspace = true }
```

## Hard rules

1. **`#![no_std]` and `#![forbid(unsafe_code)]`.** No exceptions.
2. **Parsing borrows and does not allocate.** Return `&'a str` / `&'a [u8]` views into the
   caller's buffer. Anything that must own — serializers, lossy decoding, `Vec` of items —
   goes behind the `alloc` feature. Prefer returning an *iterator* over a collection: a file
   list, an ID list, and a set of RTF runs are all iterable without allocating.
3. **Read through `rclip_core::Reader`.** Never index a slice with a value that came from the
   input. `Reader` exists precisely so `buf[off..off + len]` never appears in a codec.
4. **Never size an allocation or a loop from a length field** without `Reader::check_count`
   first. A `u32` count off the wire is a one-line OOM.
5. **Bound recursion** at `rclip_core::MAX_DEPTH` and return `ErrorKind::DepthLimit`. Never
   overflow the stack.
6. **Parsers return data.** No path resolution, no filesystem access, no launching anything.
   This applies with particular force to `.lnk` and `.desktop`.
7. **Minimal dependencies.** `rclip-core` only, unless you have justified an addition (below).

## Before writing code: check crates.io

Search for an existing crate that already does this format. Then judge it on:

- **Dependency chain.** Run `cargo tree` on a scratch project. More than ~3 transitive deps, or
  anything pulling `syn`/`serde_derive`/`regex`, is too much for a codec this small.
- **`no_std`.** If it needs `std`, it is probably not usable here.
- **Correctness on the parts that matter.** The plan calls out the specific hard parts per
  format (RTF's `\ucN` skip counter, CF_HTML's offsets, desktop-entry escapes). If the crate
  gets those wrong, it does not save any work.

Record the verdict in your crate's `README.md` under a `## Prior art` heading — which crates you
looked at, and one line on why you did or didn't use each. A "we wrote our own because X" note is
as valuable as reusing.

If you do take a dependency, add it to the root `Cargo.toml` `[workspace.dependencies]` and
reference it with `{ workspace = true }`.

## Test fixtures

Synthetic Phase-0 fixtures live in `corpus/synthetic/<crate-name>/`:

```
corpus/synthetic/rclip-dropfiles/
├── two-paths-wide.bin
├── two-paths-wide.json      sidecar, see below
├── empty.bin
└── truncated-header.bin     malformed input is a fixture too
```

Every `.bin` gets a `.json` sidecar:

```json
{
  "format": "CF_HDROP",
  "origin": "synthetic",
  "description": "Two absolute paths, fWide=1, drop point (0,0)",
  "expect": "ok",
  "notes": "Hand-built to the DROPFILES layout in MS docs"
}
```

`"expect"` is `"ok"` or `"error"`. Include **at least one malformed fixture per crate** — a
truncated header, a length field pointing past the end, a bad magic — and assert the parser
returns the right `ErrorKind` rather than panicking. That is the fixture that matters most.

`"origin"` is `"synthetic"` for hand-built bytes. Real captures from real applications land in
`corpus/<platform>/<app>/` later, in Phase 1; leave that alone for now.

Load fixtures in tests with a relative path from `CARGO_MANIFEST_DIR`:

```rust
fn fixture(name: &str) -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/synthetic/");
    std::fs::read(format!("{p}rclip-dropfiles/{name}")).expect("fixture")
}
```

Integration tests may use `std` — only the library is `no_std`.

## Scope for Phase 0

Types, the parser, and tests. A serializer only where the plan calls it out as load-bearing
(shell-link, cf-html, dropfiles). Where something is deliberately unimplemented, say so with a
`// TODO(phase-N):` comment and a line in the crate README — not a silent gap.

## Definition of done

- `cargo test -p rclip-<name>` passes.
- `cargo build -p rclip-<name> --no-default-features` passes (proves `no_std` holds).
- `cargo clippy -p rclip-<name> -- -D warnings` is clean.
- README states the spec, the prior-art verdict, and what is not implemented yet.

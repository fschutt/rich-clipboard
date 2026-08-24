#!/usr/bin/env python3
"""Generate `crates/rclip-codepage/src/tables.rs` from the Unicode Consortium's
vendor mapping files.

The tables in this crate are not typed by hand. A single transposed digit in a
128-entry table is invisible in review and produces mojibake in exactly one
locale, so the tables are derived mechanically from the authoritative source and
this script is checked in so the derivation is auditable.

Usage
-----

    python3 generate.py                 # fetch from unicode.org, verify, emit
    python3 generate.py --source-dir D  # read the .TXT files from D instead
    python3 generate.py --check         # emit to stdout, do not touch the tree
    python3 generate.py --update-hashes # re-pin SHA-256s after an upstream change

Every source file is pinned by SHA-256. A mismatch is a hard error rather than a
warning: if unicode.org revises a mapping, that must show up as a deliberate
commit that re-pins the hash, not as a silent change in what this crate decodes.

Source format (Unicode "Format A"): tab-separated `0xNN <tab> 0xNNNN <tab>
#NAME`, one line per byte value. A line whose second column is blank marks a
byte the code page leaves *undefined* -- CP1252 0x81 is the canonical example.
Those become the sentinel 0 in the generated table, which the decoder reports as
`None` rather than silently substituting U+FFFD.
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import re
import sys
import urllib.request

BASE = "https://www.unicode.org/Public/MAPPINGS/VENDORS/"

# name, url suffix, Rust const identifier, pinned sha256 of the fetched file.
SOURCES = [
    ("CP1250", "MICSFT/WINDOWS/CP1250.TXT", "WINDOWS_1250",
     "e6535e3c81f4aff1a4b369c46e588a2423d6a75da341e01a6cea108c7542c19a"),
    ("CP1251", "MICSFT/WINDOWS/CP1251.TXT", "WINDOWS_1251",
     "d9d491808bd7956c26ef8ed07fa63015bfab32860720e07d5bf3a64a609927ee"),
    ("CP1252", "MICSFT/WINDOWS/CP1252.TXT", "WINDOWS_1252",
     "f607ae328b4dff5e9bfef725f5fff0ae23f38797f8a5b95998a0d2735c0e8fad"),
    ("CP1253", "MICSFT/WINDOWS/CP1253.TXT", "WINDOWS_1253",
     "85d917854926ef09b22abefd19c7b61494e229fa391b648dc332f1fac0879ce3"),
    ("CP1254", "MICSFT/WINDOWS/CP1254.TXT", "WINDOWS_1254",
     "f93569399f81abce90441793c118d2524770b58d9c5774de3233c64d88c9fca0"),
    ("CP1255", "MICSFT/WINDOWS/CP1255.TXT", "WINDOWS_1255",
     "67ab4e3e7c088be41e0f230fa9c2c5b9a803395bec8027a5ee77df74432290c7"),
    ("CP1256", "MICSFT/WINDOWS/CP1256.TXT", "WINDOWS_1256",
     "fd5835c5e3be668d4fa3dc3d0d3a6491795646fc9629c0892f45dd08aa72f367"),
    ("CP1257", "MICSFT/WINDOWS/CP1257.TXT", "WINDOWS_1257",
     "5ac72f382ea4f64fe61854e0dae7d311583bd1dd20797697d4dce32b175a268b"),
    ("CP1258", "MICSFT/WINDOWS/CP1258.TXT", "WINDOWS_1258",
     "2457d5d0fbb1d444136c579edc488d0a34474d58a36b6eb2adacba88c068df2e"),
    ("CP437", "MICSFT/PC/CP437.TXT", "CP437",
     "6bad4dabcdf5940227c7d81fab130dcb18a77850b5d79de28b5dc4e047b0aaac"),
    ("CP850", "MICSFT/PC/CP850.TXT", "CP850",
     "ffdcc3c1c72f1aef600a63547100ef3dc452a09ad84923d382085519751c7479"),
    ("ROMAN", "APPLE/ROMAN.TXT", "MAC_ROMAN",
     "18e571645be895e9553ed5c842ea8f65f9c5d3c9ccb43e66e0c33a132ed0d721"),
]

OUT = pathlib.Path(__file__).resolve().parent.parent / "src" / "tables.rs"
HERE = pathlib.Path(__file__).resolve()

LINE = re.compile(r"^0x([0-9A-Fa-f]{2})\t\s*(0x([0-9A-Fa-f]{1,6}))?")

# Doc line per table, written above the const in the generated file.
DOCS = {
    "WINDOWS_1250": "Windows-1250, Central European (Latin 2).",
    "WINDOWS_1251": "Windows-1251, Cyrillic.",
    "WINDOWS_1252": "Windows-1252, Western European (Latin 1).",
    "WINDOWS_1253": "Windows-1253, Greek.",
    "WINDOWS_1254": "Windows-1254, Turkish (Latin 5).",
    "WINDOWS_1255": "Windows-1255, Hebrew.",
    "WINDOWS_1256": "Windows-1256, Arabic.",
    "WINDOWS_1257": "Windows-1257, Baltic.",
    "WINDOWS_1258": "Windows-1258, Vietnamese.",
    "CP437": "IBM/OEM code page 437, the original US PC-DOS character set.",
    "CP850": "IBM/OEM code page 850, DOS Latin-1.",
    "MAC_ROMAN": "Mac OS Roman.",
}


def fetch(suffix: str, source_dir: pathlib.Path | None, name: str) -> bytes:
    if source_dir is not None:
        return (source_dir / f"{name}.TXT").read_bytes()
    url = BASE + suffix
    with urllib.request.urlopen(url, timeout=60) as r:  # noqa: S310 - fixed https URL
        return r.read()


def parse_table(name: str, raw: bytes) -> tuple[list[int], str]:
    """Return the 0x80..=0xFF half of the mapping, plus the upstream revision line.

    Only the high half is kept: all twelve of these encodings are verified
    ASCII-transparent below 0x80 (the check below is what makes that claim
    load-bearing rather than folklore), so 128 entries per encoding is the whole
    table and the low half needs no storage at all.
    """
    text = raw.decode("ascii", errors="replace")
    mapping: dict[int, int | None] = {}
    for line in text.splitlines():
        m = LINE.match(line)
        if not m:
            continue
        b = int(m.group(1), 16)
        mapping[b] = int(m.group(3), 16) if m.group(3) else None

    # ASCII transparency. Mac OS Roman's file omits 0x00..0x1F and 0x7F entirely
    # -- its own header says those are "the standard control characters" -- so a
    # missing low byte is identity, but a *present* low byte that maps somewhere
    # other than itself would invalidate the 128-entry layout.
    for b in range(0x80):
        u = mapping.get(b, b)
        if u != b:
            raise SystemExit(f"{name}: byte {b:#04x} is not ASCII-transparent (-> {u:#06x})")

    table: list[int] = []
    for b in range(0x80, 0x100):
        if b not in mapping:
            raise SystemExit(f"{name}: no line for byte {b:#04x}")
        u = mapping[b]
        if u is None:
            # Undefined in this code page. Sentinel 0 -- safe because U+0000 is
            # only ever the target of byte 0x00, which is not in the high half.
            table.append(0)
            continue
        if u == 0:
            raise SystemExit(f"{name}: byte {b:#04x} maps to U+0000, which is the sentinel")
        if u > 0xFFFF:
            raise SystemExit(f"{name}: byte {b:#04x} maps to {u:#x}, outside the BMP")
        if 0xD800 <= u <= 0xDFFF:
            raise SystemExit(f"{name}: byte {b:#04x} maps to surrogate {u:#06x}")
        table.append(u)

    # A duplicated target would make the reverse (encode) lookup ambiguous. None
    # of these twelve has one; assert it rather than assume it.
    seen: dict[int, int] = {}
    for i, u in enumerate(table):
        if u == 0:
            continue
        if u in seen:
            raise SystemExit(
                f"{name}: U+{u:04X} is the target of both "
                f"{seen[u] + 0x80:#04x} and {i + 0x80:#04x}"
            )
        seen[u] = i

    revision = upstream_revision(text)
    return table, revision


def upstream_revision(text: str) -> str:
    """The line that identifies which revision of the file this is.

    Microsoft's files carry `Table version:` and `Date:`; Apple's carry a
    changes log whose first entry is the revision. Both are recorded verbatim in
    the generated file so a future reader can tell which vintage of the mapping
    the constants came from.
    """
    version = date = None
    for line in text.splitlines():
        if version is None and "Table version:" in line:
            version = line.split("Table version:", 1)[1].strip()
        if date is None and re.search(r"^#\s+Date:", line):
            date = line.split("Date:", 1)[1].strip()
        # Apple: "#       c02  2005-Apr-05    Update header comments."
        m = re.match(r"^#\s+([a-z]\d+)\s+(\d{4}-[A-Za-z]{3}-\d{2})\s", line)
        if m and version is None:
            version, date = m.group(1), m.group(2)
            break
    if version and date:
        return f"{version} ({date})"
    return version or date or "unknown"


def render(entries: list[tuple[str, str, list[int], str, str]]) -> str:
    out: list[str] = []
    out.append("// @generated by crates/rclip-codepage/generate/generate.py -- do not edit.")
    out.append("//")
    out.append("// Regenerate with:")
    out.append("//     python3 crates/rclip-codepage/generate/generate.py")
    out.append("")
    out.append("//! Byte-to-Unicode tables, generated from the Unicode Consortium's vendor")
    out.append("//! mapping files.")
    out.append("//!")
    out.append("//! Each table covers `0x80..=0xFF` only. Every encoding in this crate is")
    out.append("//! ASCII-transparent below `0x80` -- the generator verifies that against the")
    out.append("//! source file rather than assuming it -- so the low half needs no storage.")
    out.append("//!")
    out.append("//! A `0` entry means the code page leaves that byte **undefined**. It is not")
    out.append("//! U+0000: no high byte in any of these tables maps to U+0000, so the value is")
    out.append("//! free to use as a sentinel. Decoders report it as `None` rather than")
    out.append("//! substituting U+FFFD, because a caller that wants lossy behaviour should have")
    out.append("//! to ask for it.")
    out.append("//!")
    out.append("//! Sources, with the upstream revision each table was generated from:")
    out.append("//!")
    for name, ident, _table, revision, sha in entries:
        suffix = next(s for n, s, _, _ in SOURCES if n == name)
        out.append(f"//! - `{ident}`, table version {revision}, sha256 `{sha[:16]}...`")
        out.append(f"//!   <{BASE}{suffix}>")
    out.append("")

    for name, ident, table, revision, _sha in entries:
        undefined = [i + 0x80 for i, u in enumerate(table) if u == 0]
        out.append(f"/// {DOCS[ident]}")
        out.append("///")
        out.append(f"/// Generated from `{name}.TXT`, table version {revision}.")
        if undefined:
            out.append("///")
            out.append(f"/// Undefined bytes ({len(undefined)}), reported as `None` rather than")
            out.append("/// substituted:")
            out.append("///")
            for row in range(0, len(undefined), 10):
                listed = ", ".join(f"`0x{b:02X}`" for b in undefined[row:row + 10])
                out.append(f"/// {listed}")
        else:
            out.append("///")
            out.append("/// Every byte is defined; this table has no `0` entries.")
        out.append(f"pub const {ident}: [u16; 128] = [")
        for row in range(0, 128, 8):
            cells = ", ".join(f"0x{u:04X}" for u in table[row:row + 8])
            out.append(f"    {cells}, // {row + 0x80:02X}-{row + 0x87:02X}")
        out.append("];")
        out.append("")

    # No trailing blank line: the emitted file must already be rustfmt-clean, or
    # regenerating it would break `cargo fmt --check` in CI.
    while out and out[-1] == "":
        out.pop()
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--source-dir", type=pathlib.Path, default=None,
                    help="read NAME.TXT from this directory instead of fetching")
    ap.add_argument("--check", action="store_true", help="write to stdout, not to the tree")
    ap.add_argument("--update-hashes", action="store_true",
                    help="rewrite this script's pinned SHA-256s from what was fetched")
    args = ap.parse_args()

    entries: list[tuple[str, str, list[int], str, str]] = []
    fresh: dict[str, str] = {}
    for name, suffix, ident, pinned in SOURCES:
        raw = fetch(suffix, args.source_dir, name)
        sha = hashlib.sha256(raw).hexdigest()
        fresh[name] = sha
        if not args.update_hashes and sha != pinned:
            print(
                f"{name}: sha256 mismatch\n  pinned {pinned}\n  actual {sha}\n"
                f"Upstream changed. Review the diff, then re-run with --update-hashes.",
                file=sys.stderr,
            )
            return 1
        table, revision = parse_table(name, raw)
        entries.append((name, ident, table, revision, sha))

    if args.update_hashes:
        src = HERE.read_text()
        for name, _suffix, _ident, pinned in SOURCES:
            src = src.replace(f'"{pinned}"', f'"{fresh[name]}"', 1)
        HERE.write_text(src)
        print("re-pinned SHA-256s; re-run without --update-hashes", file=sys.stderr)
        return 0

    text = render(entries) + "\n"
    if args.check:
        sys.stdout.write(text)
    else:
        OUT.write_text(text)
        print(f"wrote {OUT} ({len(entries)} tables)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

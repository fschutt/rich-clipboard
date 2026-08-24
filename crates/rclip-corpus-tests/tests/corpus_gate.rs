//! Deliverable A: the corpus as one checked thing.
//!
//! Every `.bin` is paired with a sidecar, every sidecar meets the contract in
//! `corpus/README.md`, and every fixture is handed to the parser that owns its
//! directory and made to produce what its sidecar promises. A fixture nobody
//! routes, a sidecar nobody reads and a `.bin` nobody loads are all failures
//! here, which is the point: a corpus that nothing checks is documentation.

use rclip_core::{Error, ErrorKind, Result};
use rclip_corpus_tests::sidecar::{Expect, Origin, Sidecar, MAX_FIXTURE_BYTES, MAX_SIDECAR_BYTES};
use rclip_corpus_tests::{corpus_root, report, sidecar, walk, Fixture};

/// Read the corpus once, with every sidecar already validated.
///
/// Returns the fixtures that parsed; sidecar problems are reported by
/// [`every_sidecar_meets_the_contract`] rather than by every test that needs a
/// sidecar.
fn corpus() -> Vec<(Fixture, Sidecar)> {
    let root = corpus_root();
    let found = walk(&root).expect("corpus/ must exist");
    found
        .fixtures
        .into_iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(&f.sidecar).ok()?;
            let s = sidecar::parse(&text).ok()?;
            Some((f, s))
        })
        .collect()
}

#[test]
fn no_orphans_in_either_direction() {
    let root = corpus_root();
    let found = walk(&root).expect("corpus/ must exist");
    let problems: Vec<String> = found.orphans.iter().map(ToString::to_string).collect();
    report(
        "corpus/ holds files that are not a .bin/.json pair",
        &problems,
    );
    assert!(
        !found.fixtures.is_empty(),
        "corpus/ is empty; the gate would pass vacuously"
    );
}

#[test]
fn every_sidecar_meets_the_contract() {
    let root = corpus_root();
    let found = walk(&root).expect("corpus/ must exist");
    let mut problems = Vec::new();
    for f in &found.fixtures {
        let text = match std::fs::read_to_string(&f.sidecar) {
            Ok(t) => t,
            Err(e) => {
                problems.push(format!("{}: {e}", f.sidecar.display()));
                continue;
            }
        };
        if let Err(found) = sidecar::parse(&text) {
            for p in found {
                problems.push(format!("{}  {p}", f.label()));
            }
        }
    }
    report("sidecars that do not meet corpus/README.md", &problems);
}

#[test]
fn no_fixture_is_enormous() {
    let root = corpus_root();
    let found = walk(&root).expect("corpus/ must exist");
    let mut problems = Vec::new();
    for f in &found.fixtures {
        for (path, limit, what) in [
            (&f.bin, MAX_FIXTURE_BYTES, "fixture"),
            (&f.sidecar, MAX_SIDECAR_BYTES, "sidecar"),
        ] {
            let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if len > limit {
                problems.push(format!(
                    "{}: {what} is {len} bytes, over the {limit}-byte limit. These are \
                     unit-test inputs; cut it down or keep it out of the repository.",
                    path.display()
                ));
            }
        }
    }
    report("fixtures that are too big for a unit test", &problems);
}

/// Every directory under `corpus/` has to be routed by [`exercise`], and every
/// crate directory has to correspond to a crate. A new corpus directory that
/// nobody parses fails here rather than sitting unread.
#[test]
fn every_directory_is_routed_to_a_parser() {
    let root = corpus_root();
    let found = walk(&root).expect("corpus/ must exist");
    let mut problems = Vec::new();
    for dir in &found.dirs {
        let leaf = dir.rsplit('/').next().unwrap_or(dir);
        if dir.starts_with("synthetic/") {
            if !is_routed_crate(leaf) {
                problems.push(format!(
                    "corpus/{dir}: no parser is wired up for {leaf:?} in \
                     tests/corpus_gate.rs::exercise"
                ));
            }
            let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("crates")
                .join(leaf);
            if !crate_dir.is_dir() {
                problems.push(format!(
                    "corpus/{dir}: there is no crates/{leaf}; a synthetic directory is named \
                     after the crate that owns it"
                ));
            }
        } else if dir.is_empty() {
            problems.push(
                "corpus/ holds fixtures at the top level; they belong in \
                 synthetic/<crate>/ or <platform>/<app>/"
                    .to_owned(),
            );
        } else if dir.split('/').count() != 2 {
            // corpus/README.md: captures live in <platform>/<app>/.
            problems.push(format!(
                "corpus/{dir}: a capture directory is <platform>/<app>/, two levels deep"
            ));
        }
    }
    report("corpus directories nothing parses", &problems);
}

#[test]
fn every_fixture_produces_what_its_sidecar_promises() {
    let mut problems = Vec::new();
    let mut routed = 0usize;
    let all = corpus();
    let total = all.len();

    for (f, s) in all {
        let bytes = match std::fs::read(&f.bin) {
            Ok(b) => b,
            Err(e) => {
                problems.push(format!("{}: {e}", f.bin.display()));
                continue;
            }
        };
        let Some(outcome) = exercise(&f, &s, &bytes) else {
            problems.push(format!(
                "{}: format {:?} in directory {:?} matches no parser in \
                 tests/corpus_gate.rs::exercise",
                f.label(),
                s.format,
                f.leaf_dir()
            ));
            continue;
        };
        routed += 1;

        match s.expect {
            Expect::Ok => {
                if !outcome.0.is_empty() {
                    problems.push(format!(
                        "{}: sidecar says \"ok\", parsers returned {}",
                        f.label(),
                        outcome.summary()
                    ));
                }
            }
            Expect::Error => {
                let declared = s.error_kind.as_ref().expect("contract requires one");
                if !outcome.has(&declared.variant) {
                    problems.push(format!(
                        "{}: sidecar's {} says {}, parsers returned {}. \
                         \"fails somehow\" is not the requirement.",
                        f.label(),
                        declared.source,
                        declared.variant,
                        outcome.summary()
                    ));
                }
            }
        }
    }

    report("fixtures that disagree with their sidecar", &problems);
    // Anti-vacuity. Tied to what the walker found rather than to a number that
    // goes stale every time the corpus grows.
    assert_eq!(
        routed, total,
        "every fixture the walker found has to be routed"
    );
    assert!(total >= 100, "corpus/ has shrunk to {total} fixtures");
}

/// A captured fixture is worth more than a hand-built one and costs more to
/// get wrong, so the metadata that makes it re-takeable is mandatory. The
/// sidecar contract already enforces `os`/`app`/`how`; this pins the one thing
/// that is about the tree rather than the file.
#[test]
fn captures_live_where_captures_live() {
    let mut problems = Vec::new();
    let mut captures_under_synthetic = 0usize;
    for (f, s) in corpus() {
        match (f.top_dir(), s.origin) {
            // Four fixtures predate the `<platform>/<app>/` layout and are
            // load-bearing for other crates' tests at the paths they are on, so
            // moving the `.bin` would break assertions elsewhere. Tolerated,
            // and counted: the number is pinned below so a fifth cannot be
            // added by copying the fourth.
            ("synthetic", Origin::Captured) => captures_under_synthetic += 1,
            ("synthetic", Origin::Synthetic) => {}
            (_, Origin::Synthetic) => problems.push(format!(
                "{}: hand-built bytes belong in corpus/synthetic/<crate>/",
                f.label()
            )),
            (_, Origin::Captured) => {}
        }
    }
    report("fixtures filed in the wrong place", &problems);
    assert_eq!(
        captures_under_synthetic, 4,
        "corpus/README.md files captures under <platform>/<app>/. Four predate that layout and \
         are pinned by other crates' tests at their current paths; a new one goes in the right \
         place rather than joining them."
    );
}

/// A capture tool that records the payload size gives the corpus a free
/// truncation check: if `bytes` and the file disagree, either the capture was
/// cut short on the way in or the `.bin` was edited afterwards, and a fixture
/// that has been edited is no longer a capture.
#[test]
fn a_recorded_byte_count_matches_the_file() {
    let mut problems = Vec::new();
    let mut checked = 0usize;
    for (f, s) in corpus() {
        let Some(rclip_corpus_tests::json::Value::Number(n)) = s.raw.get("bytes") else {
            continue;
        };
        let Ok(declared) = n.parse::<u64>() else {
            problems.push(format!(
                "{}: \"bytes\" is {n:?}, not a byte count",
                f.label()
            ));
            continue;
        };
        let actual = std::fs::metadata(&f.bin).map(|m| m.len()).unwrap_or(0);
        if declared != actual {
            problems.push(format!(
                "{}: sidecar records {declared} bytes, the file is {actual}",
                f.label()
            ));
        }
        checked += 1;
    }
    report(
        "captures whose recorded size does not match the file",
        &problems,
    );
    println!("{checked} fixtures record a byte count");
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

fn is_routed_crate(leaf: &str) -> bool {
    matches!(
        leaf,
        "rclip-bookmark"
            | "rclip-cf-html"
            | "rclip-codepage"
            | "rclip-desktop-entry"
            | "rclip-dib"
            | "rclip-dropfiles"
            | "rclip-file-desc"
            | "rclip-html"
            | "rclip-idlist"
            | "rclip-rtf"
            | "rclip-shell-link"
            | "rclip-uri-list"
            | "rclip-url-file"
            | "rclip-webloc"
    )
}

/// Everything a traversal went wrong on.
///
/// Not just the first failure. Several of these formats are explicitly
/// designed so that one broken part does not cost you the rest — a `CIDA`
/// child with a bad offset must not poison its parent, a bookmark entry that
/// points past the end must not invalidate the entry before it — so "the error
/// this fixture is about" is not always the first one a walk trips over. The
/// gate therefore asks whether the declared kind is *among* what the parsers
/// produced, and prints the whole list when it is not.
#[derive(Debug, Default)]
struct Errors(Vec<Error>);

impl Errors {
    fn watch<T>(&mut self, r: Result<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.0.push(e);
                None
            }
        }
    }

    fn has(&self, kind: &str) -> bool {
        self.0.iter().any(|e| format!("{:?}", e.kind) == kind)
    }

    fn summary(&self) -> String {
        if self.0.is_empty() {
            return "nothing at all".to_owned();
        }
        self.0
            .iter()
            .map(|e| format!("{:?}@{}", e.kind, e.offset))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Run a fixture through the parser its directory names, as deep as the format
/// goes.
///
/// Returns `None` when nothing is wired up for it.
///
/// Each arm goes past the entry point on purpose: several fixtures are
/// structurally fine and only fail when a value is decoded or a list is walked,
/// and a gate that stopped at `parse()` would call those "declared error but
/// parsed cleanly". The deep walk is also what makes `expect: "ok"` mean
/// something — an `Ok` here is a fixture every accessor agreed on, not one that
/// survived a header check.
fn exercise(f: &Fixture, s: &Sidecar, bytes: &[u8]) -> Option<Errors> {
    let mut e = Errors::default();
    match f.leaf_dir() {
        "rclip-bookmark" => bookmark(&mut e, bytes),
        "rclip-cf-html" => drop(e.watch(rclip_cf_html::parse(bytes))),
        "rclip-codepage" => codepage(&mut e, s, bytes)?,
        "rclip-desktop-entry" => desktop_entry(&mut e, bytes),
        "rclip-dib" => drop(e.watch(rclip_dib::decode(bytes, rclip_dib::AlphaMode::Guess))),
        "rclip-dropfiles" => dropfiles(&mut e, bytes),
        "rclip-file-desc" => file_desc(&mut e, &s.format, bytes),
        "rclip-html" => html_fragment(&mut e, bytes),
        "rclip-idlist" => idlist(&mut e, &s.format, bytes),
        "rclip-rtf" => drop(e.watch(rclip_rtf::Document::parse(bytes))),
        "rclip-shell-link" => shell_link(&mut e, bytes),
        "rclip-uri-list" => uri_list(&mut e, &s.format, bytes),
        "rclip-url-file" => url_file(&mut e, bytes),
        "rclip-webloc" => drop(e.watch(rclip_webloc::Webloc::parse(bytes))),
        // A capture directory is named after an application, which does not
        // name a format; those route on what the sidecar says they are.
        _ => route_capture(&mut e, s, bytes)?,
    }
    Some(e)
}

/// Route a capture: by `flavor` where the sidecar names one, since that is this
/// workspace's own vocabulary, and by `format` otherwise.
///
/// Neither is the directory name, so `corpus/macos/Safari/` works without this
/// file having to know that Safari exists.
fn route_capture(e: &mut Errors, s: &Sidecar, bytes: &[u8]) -> Option<()> {
    if let Some(flavor) = s.raw.get("flavor").and_then(|v| v.as_str()) {
        match flavor {
            "Rtf" => {
                e.watch(rclip_rtf::Document::parse(bytes));
                return Some(());
            }
            "Html" => {
                html(e, bytes);
                return Some(());
            }
            "PlainText" => {
                plain_text_utf8(e, bytes);
                return Some(());
            }
            "Dib" | "DibV5" => {
                e.watch(rclip_dib::decode(bytes, rclip_dib::AlphaMode::Guess));
                return Some(());
            }
            "Png" => {
                magic(e, bytes, b"\x89PNG\r\n\x1a\n");
                return Some(());
            }
            "Tiff" => {
                tiff(e, bytes);
                return Some(());
            }
            "ShellLink" => {
                shell_link(e, bytes);
                return Some(());
            }
            "ShellIdList" => {
                idlist(e, "CFSTR_SHELLIDLIST", bytes);
                return Some(());
            }
            "FileDescriptor" => {
                file_desc(e, &s.format, bytes);
                return Some(());
            }
            // `FileList` and `Url` are several formats wearing one name, so
            // they fall through to the `format` string, which distinguishes
            // `CF_HDROP` from `text/uri-list` from `public.file-url`.
            _ => {}
        }
    }
    route_by_format(e, &s.format, bytes)
}

/// `Flavor::Html` is two different payloads. On Windows it is `CF_HTML`, with
/// a `Version:` header and byte offsets; everywhere else it is bare markup,
/// which the CF_HTML parser correctly refuses with `BadMagic`. Sniffing for the
/// header is the right thing *here* and the wrong thing in the codec: the
/// flavour name says "HTML" and does not say which of the two spellings
/// arrived, so something has to look.
fn html(e: &mut Errors, bytes: &[u8]) {
    let head = &bytes[..bytes.len().min(64)];
    if head.windows(8).any(|w| w.eq_ignore_ascii_case(b"version:")) {
        e.watch(rclip_cf_html::parse(bytes));
    } else {
        plain_text_utf8(e, bytes);
    }
}

/// Check a leading signature.
///
/// `rich-clipboard` decodes no image format but `CF_DIB`: PNG, JPEG, GIF and
/// TIFF all arrive as a self-describing file and go straight to an image
/// decoder that is not this workspace's business. Checking the signature is
/// still worth doing — it is what catches a capture that grabbed the wrong
/// flavour, or grabbed nothing.
fn magic(e: &mut Errors, bytes: &[u8], sig: &[u8]) {
    if !bytes.starts_with(sig) {
        e.0.push(Error::new(ErrorKind::BadMagic, 0));
    }
}

/// TIFF is the one signature with two spellings: `II*\0` little-endian and
/// `MM\0*` big-endian. NSPasteboard hands out the big-endian one.
fn tiff(e: &mut Errors, bytes: &[u8]) {
    if !bytes.starts_with(b"II*\x00") && !bytes.starts_with(b"MM\x00*") {
        e.0.push(Error::new(ErrorKind::BadMagic, 0));
    }
}

/// A format this workspace deliberately does not decode.
///
/// Routing it to nothing would let it sit in the corpus unchecked, which is the
/// hole this gate exists to close, so it still has to be non-empty and it is
/// still leak-scanned and size-checked like everything else. Each one is listed
/// by name in `route_by_format` with a reason, so "no codec claims this" is a
/// decision somebody wrote down rather than an omission.
fn not_decoded(e: &mut Errors, bytes: &[u8]) {
    if bytes.is_empty() {
        e.0.push(Error::new(ErrorKind::UnexpectedEof, 0));
    }
}

/// UTF-8 text is not a codec, but "these bytes decode" is still the whole
/// correctness question for a text flavour, and a capture that claims `ok`
/// should have to answer it.
fn plain_text_utf8(e: &mut Errors, bytes: &[u8]) {
    if let Err(err) = std::str::from_utf8(bytes) {
        e.0.push(Error::new(ErrorKind::InvalidUtf8, err.valid_up_to()));
    }
}

/// `public.utf16-external-plain-text` and its `ut16` OSType alias. "External"
/// means a byte-order mark leads, which is also the only thing that says which
/// way round the units are.
fn plain_text_utf16(e: &mut Errors, bytes: &[u8]) {
    let (body, big_endian, base) = match bytes {
        [0xFF, 0xFE, rest @ ..] => (rest, false, 2),
        [0xFE, 0xFF, rest @ ..] => (rest, true, 2),
        // No BOM. NSPasteboard writes one; without it, little-endian is the
        // only sane guess on every machine this repository targets.
        _ => (bytes, false, 0),
    };
    if body.len() % 2 != 0 {
        e.0.push(Error::new(ErrorKind::InvalidUtf16, base + body.len() - 1));
        return;
    }
    let units = body.chunks_exact(2).map(|c| {
        if big_endian {
            u16::from_be_bytes([c[0], c[1]])
        } else {
            u16::from_le_bytes([c[0], c[1]])
        }
    });
    for (i, c) in char::decode_utf16(units).enumerate() {
        if c.is_err() {
            e.0.push(Error::new(ErrorKind::InvalidUtf16, base + i * 2));
            return;
        }
    }
}

/// A `CorePasteboardFlavorType 0xNNNNNNNN` name is a four-character OSType in
/// hex. Decoding it is how `0x75743136` becomes `ut16`, which is a format name
/// rather than a magic number worth hardcoding.
fn ostype_of(format: &str) -> Option<String> {
    let hex = format
        .to_ascii_lowercase()
        .strip_prefix("corepasteboardflavortype 0x")?
        .to_owned();
    if hex.len() != 8 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(&hex, 16).ok()?;
    String::from_utf8(n.to_be_bytes().to_vec()).ok()
}

/// Route a capture by its declared `format`.
fn route_by_format(e: &mut Errors, format: &str, bytes: &[u8]) -> Option<()> {
    let f = ostype_of(format).unwrap_or_else(|| format.to_ascii_lowercase());
    match f.as_str() {
        "public.rtf" | "next rich text format v1.0 pasteboard type" | "rtf " => {
            e.watch(rclip_rtf::Document::parse(bytes));
        }
        "public.utf8-plain-text" | "nsstringpboardtype" | "utf8" => plain_text_utf8(e, bytes),
        "public.utf16-external-plain-text" | "ut16" => plain_text_utf16(e, bytes),
        "public.html" | "apple html pasteboard type" | "text/html" => html(e, bytes),
        // A `.webarchive` is a binary plist whose root dict holds
        // WebMainResource. This workspace has no webarchive codec, but it does
        // have a bplist reader, and "the container is structurally sound" is
        // the part worth checking.
        "com.apple.webarchive" | "apple web archive pasteboard type" => {
            e.watch(rclip_webloc::BinaryPlist::parse(bytes));
        }
        // WebKit's private cross-process pasteboard bookkeeping: a length, a
        // tag, and an origin UUID. Undocumented, WebKit-internal, and nothing
        // in layer 2 will ever decode it.
        "com.apple.webkit.custom-pasteboard-data" => not_decoded(e, bytes),
        // A single `file://` URL, which is a one-line `text/uri-list` in every
        // way that matters to a parser.
        "public.file-url" | "public.url" => uri_list(e, "text/uri-list", bytes),
        "public.png" | "apple png pasteboard type" | "png " | "pngf" => {
            magic(e, bytes, b"\x89PNG\r\n\x1a\n");
        }
        "public.tiff" | "next tiff v4.0 pasteboard type" | "tiff" => tiff(e, bytes),
        "public.jpeg" | "jpeg" => magic(e, bytes, b"\xff\xd8\xff"),
        // Preview's own bookkeeping, offered under a private name and again
        // under the `dyn.` UTI that stands in for one. A binary plist, so the
        // container is checkable even though its contents are Preview's.
        "pvpboardinfopboardtype" => {
            e.watch(rclip_webloc::BinaryPlist::parse(bytes));
        }
        _ if f.starts_with("dyn.") => {
            // A `dyn.` UTI is a base32 encoding of a type nobody registered.
            // Sniff the container rather than pretend to know the type.
            if bytes.starts_with(b"bplist00") {
                e.watch(rclip_webloc::BinaryPlist::parse(bytes));
            } else {
                not_decoded(e, bytes);
            }
        }
        "bookmarkdata" => bookmark(e, bytes),
        "cf_html" => drop(e.watch(rclip_cf_html::parse(bytes))),
        "cf_dib" | "cf_dibv5" => {
            drop(e.watch(rclip_dib::decode(bytes, rclip_dib::AlphaMode::Guess)));
        }
        "cf_hdrop" => dropfiles(e, bytes),
        "cfstr_filedescriptorw" => file_desc(e, "CFSTR_FILEDESCRIPTORW", bytes),
        "cfstr_filedescriptor" | "filegroupdescriptora" => {
            file_desc(e, "FILEGROUPDESCRIPTORA", bytes);
        }
        "itemidlist" | "cfstr_shellidlist" => idlist(e, format, bytes),
        "ms-shllink" => shell_link(e, bytes),
        "webloc" | "inetloc" => drop(e.watch(rclip_webloc::Webloc::parse(bytes))),
        _ if f.starts_with("rtf ") || f.starts_with("rtf1") => {
            e.watch(rclip_rtf::Document::parse(bytes));
        }
        _ if f.starts_with("text/uri-list") || f.starts_with("x-special/") => {
            uri_list(e, format, bytes);
        }
        _ if f.starts_with("application/x-desktop") => desktop_entry(e, bytes),
        _ if f.starts_with("application/x-mswinurl") => url_file(e, bytes),
        _ => return None,
    }
    Some(())
}

fn bookmark(e: &mut Errors, bytes: &[u8]) {
    let Some(bm) = e.watch(rclip_bookmark::Bookmark::parse(bytes)) else {
        return;
    };
    e.watch(bm.validate());
    // The lazy accessors resolve records that `validate` only counts.
    e.watch(bm.target_url());
    e.watch(bm.target_filename());
    e.watch(bm.volume_name());
    if let Some(Some(components)) = e.watch(bm.path_components()) {
        for c in components {
            e.watch(c);
        }
    }
}

/// A code page fixture is a byte string plus the number that says how to read
/// it, and the number lives in the sidecar because it is not in the payload —
/// which is the entire reason this crate exists.
fn codepage(e: &mut Errors, s: &Sidecar, bytes: &[u8]) -> Option<()> {
    let n: u32 = match s.raw.get("expect_codepage") {
        Some(rclip_corpus_tests::json::Value::Number(n)) => n.parse().ok()?,
        _ => return None,
    };
    let enc = rclip_codepage::Encoding::from_windows_codepage(n)?;
    for c in enc.decode(bytes) {
        e.watch(c);
    }
    Some(())
}

fn desktop_entry(e: &mut Errors, bytes: &[u8]) {
    let Some(file) = e.watch(rclip_desktop_entry::parse(bytes)) else {
        return;
    };
    // §4 escapes are decoded per value, so a dangling `\` at the end of a
    // `Name=` is invisible to `parse` and fatal to the reader that displays it.
    for group in file.groups() {
        for entry in group.entries() {
            for c in entry.value.chars() {
                e.watch(c);
            }
        }
    }
}

fn dropfiles(e: &mut Errors, bytes: &[u8]) {
    if let Some(drop) = e.watch(rclip_dropfiles::DropFiles::parse(bytes)) {
        for path in drop.paths() {
            let _ = path;
        }
    }
}

fn file_desc(e: &mut Errors, format: &str, bytes: &[u8]) {
    // CFSTR_FILEDESCRIPTOR comes in two layouts that differ only in the width
    // of `cFileName` — 592 bytes per descriptor wide, 332 ANSI — and neither
    // carries a marker saying which it is. The format name is the only thing
    // that decides, which is exactly why the crate refuses to sniff: an ANSI
    // payload "fits" the wide reading and comes back with a wrong name rather
    // than an error. Routing on the directory alone would have made an ANSI
    // fixture unrepresentable in the corpus.
    if format.eq_ignore_ascii_case("CFSTR_FILEDESCRIPTOR")
        || format.eq_ignore_ascii_case("FILEGROUPDESCRIPTORA")
    {
        if let Some(group) = e.watch(rclip_file_desc::FileGroupDescriptorA::parse(bytes)) {
            for d in &group {
                let _ = d;
            }
        }
        return;
    }
    if let Some(group) = e.watch(rclip_file_desc::FileGroupDescriptor::parse(bytes)) {
        for d in &group {
            let _ = d;
        }
    }
}

fn idlist(e: &mut Errors, format: &str, bytes: &[u8]) {
    if format.eq_ignore_ascii_case("CFSTR_SHELLIDLIST") {
        let Some(cida) = e.watch(rclip_idlist::Cida::parse(bytes)) else {
            return;
        };
        // Children first: a bad `aoffset` entry is what a CIDA fixture is
        // usually about, and the parent walk of the same fixture can trip over
        // the fallout of that same bad table.
        for child in cida.children() {
            if let Some(list) = e.watch(child) {
                for item in list {
                    e.watch(item);
                }
            }
        }
        if let Some(parent) = e.watch(cida.parent()) {
            for item in parent {
                e.watch(item);
            }
        }
        return;
    }
    for item in rclip_idlist::ItemIdList::new(bytes) {
        if let Some(item) = e.watch(item) {
            let _ = item.parse();
        }
    }
}

fn shell_link(e: &mut Errors, bytes: &[u8]) {
    if let Some(link) = e.watch(rclip_shell_link::ShellLink::parse(bytes)) {
        for block in link.extra_data() {
            e.watch(block);
        }
    }
}

fn uri_list(e: &mut Errors, format: &str, bytes: &[u8]) {
    let f = format.to_ascii_lowercase();
    if f.starts_with("x-special/gnome-copied-files") {
        e.watch(rclip_uri_list::convention::parse_copied_files(bytes));
        return;
    }
    if f.starts_with("application/x-kde-cutselection") {
        // Infallible by construction: one byte, and anything that is not '1'
        // is a copy. There is no error path to exercise.
        let _ = rclip_uri_list::convention::parse_kde_cut_selection(bytes);
        return;
    }
    if f.starts_with("text/plain") {
        e.watch(rclip_uri_list::convention::parse_nautilus_text_clipboard(
            bytes,
        ));
        return;
    }
    if let Some(list) = e.watch(rclip_uri_list::parse(bytes)) {
        // A truncated `%` escape is only visible once a URI is looked at.
        e.watch(list.validate_percent_encoding());
    }
}

/// An HTML fragment, as deep as the format goes: the element stack, and then
/// every text run decoded and every attribute of every tag walked.
///
/// The second pass matters because `Document::parse` is allowed to absorb
/// almost everything — a fragment that produced no error is only interesting if
/// the accessors agree, and the attribute walk is what reaches the CSS splitter
/// and the character-reference decoder on a fixture whose styling nothing else
/// looks at.
fn html_fragment(e: &mut Errors, bytes: &[u8]) {
    if e.watch(rclip_html::Document::parse(bytes)).is_none() {
        return;
    }
    for run in rclip_html::Runs::new(bytes) {
        let Some(run) = e.watch(run) else { return };
        if let rclip_html::RunText::Text(t) = run.text {
            assert!(
                t.chars().count() > 0 || t.as_raw().is_empty(),
                "a run was emitted for text that decodes to nothing"
            );
        }
    }
    for token in rclip_html::Tokenizer::new(bytes) {
        if let rclip_html::Token::StartTag(tag) = token {
            for attr in tag.attributes() {
                let _ = attr.value.chars().count();
                for decl in rclip_html::css::declarations(attr.value.as_raw()) {
                    let _ = rclip_html::css::color(decl.value);
                    let _ = rclip_html::css::font_size_pt(decl.value, None);
                }
            }
        }
    }
}

fn url_file(e: &mut Errors, bytes: &[u8]) {
    if let Some(file) = e.watch(rclip_url_file::parse(bytes)) {
        // URL is the only key the format requires, and a `.url` without one is
        // structurally fine and semantically nothing.
        e.watch(file.require_url());
    }
}

/// A sanity check on the router itself: an obviously wrong payload handed to
/// each parser must fail, so that an `exercise` arm which quietly accepts
/// everything cannot make the whole gate vacuous.
#[test]
fn the_router_is_not_a_rubber_stamp() {
    let junk = b"not a clipboard payload at all, in any format".as_slice();
    let mut accepted = Vec::new();
    for (leaf, format) in [
        ("rclip-bookmark", "BookmarkData"),
        ("rclip-cf-html", "CF_HTML"),
        ("rclip-dib", "CF_DIB"),
        ("rclip-file-desc", "CFSTR_FILEDESCRIPTORW"),
        ("rclip-rtf", "RTF 1.9.1"),
        ("rclip-shell-link", "MS-SHLLINK"),
        ("rclip-webloc", "webloc"),
    ] {
        let f = Fixture {
            bin: std::path::PathBuf::from("junk.bin"),
            sidecar: std::path::PathBuf::from("junk.json"),
            dir: format!("synthetic/{leaf}"),
            stem: "junk".into(),
        };
        let s = sidecar::parse(&format!(
            r#"{{"format":"{format}","origin":"synthetic","description":"d","expect":"ok"}}"#
        ))
        .expect("hand-built sidecar");
        if exercise(&f, &s, junk).expect("routed").0.is_empty() {
            accepted.push(format!("{leaf} accepted 45 bytes of English prose"));
        }
    }
    report("parsers that accept anything", &accepted);
}

/// The sidecar contract and `rclip-core` have to agree on the error vocabulary,
/// or a sidecar could name a kind no parser can return and the gate would never
/// notice.
#[test]
fn kinds_round_trip_through_their_names() {
    let e = Error::new(ErrorKind::BadOffset, 7);
    assert_eq!(format!("{:?}", e.kind), "BadOffset");
    assert_eq!(sidecar::canonical_kind(e.kind.as_str()), Some("BadOffset"));
    assert_eq!(sidecar::canonical_kind("BadOffset"), Some("BadOffset"));
}

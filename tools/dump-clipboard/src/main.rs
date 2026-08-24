//! Dump every format the system clipboard is currently offering.
//!
//! This is how the captured half of the corpus gets made, and it is the tool
//! you reach for whenever a paste misbehaves: it answers "what did that
//! application *actually* put on the clipboard" without guessing.
//!
//! The four backends live in [`backend`], one per platform, each gated on
//! `#[cfg(target_os = ...)]` with its own target-specific dependencies so the
//! crate builds everywhere and links only what it needs. Everything above
//! them — resolving identifiers to [`Flavor`]s, naming files, writing
//! sidecars, printing the table — is shared, so the backends cannot disagree
//! about anything that is not genuinely platform-specific.
//!
//! Unlike the codec crates this is a *tool*: it links real OS APIs, so it uses
//! `unsafe` where Win32 and AppKit require it and it is `publish = false`.

mod backend;
mod capture;
mod cli;
mod naming;
mod sidecar;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use capture::{Body, Capture, Offered, Selection};
use naming::Namer;
use rclip_core::{Flavor, Platform};
use sidecar::Value;

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Parsed::Print(text)) => {
            print!("{text}");
            return ExitCode::SUCCESS;
        }
        Ok(cli::Parsed::Run(a)) => a,
        Err(msg) => {
            eprintln!("dump-clipboard: {msg}");
            return ExitCode::from(2);
        }
    };

    if args.selection == Selection::Primary && !backend::HAS_PRIMARY_SELECTION {
        eprintln!(
            "dump-clipboard: --primary is an X11/Wayland concept; {} has one clipboard",
            backend::NAME
        );
        return ExitCode::from(2);
    }

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dump-clipboard: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &cli::Args) -> capture::Result<()> {
    let cap = backend::capture(args.selection)?;

    let mut namer = Namer::new();
    let rows: Vec<Row<'_>> = cap
        .offered
        .iter()
        .map(|offered| Row {
            stem: namer.stem(&offered.native, offered.item),
            flavor: flavor_label(&offered.native, cap.platform),
            offered,
        })
        .collect();

    let mut written = 0usize;
    if let Some(dir) = &args.out {
        written = write_all(Path::new(dir), &rows, args, &cap)?;
    }

    report(&rows, &cap, args, written)?;
    Ok(())
}

/// One line of the summary table, and one `.bin`/`.json` pair.
struct Row<'a> {
    offered: &'a Offered,
    stem: String,
    /// `None` when the identifier is not in `rclip-core`'s registry.
    flavor: Option<&'static str>,
}

// -- writing ------------------------------------------------------------------

fn write_all(
    dir: &Path,
    rows: &[Row<'_>],
    args: &cli::Args,
    cap: &Capture,
) -> capture::Result<usize> {
    std::fs::create_dir_all(dir)?;

    // Check the whole set before writing any of it: a capture that half-lands
    // on top of a previous one is worse than a capture that refuses to start.
    if !args.force {
        let clashes: Vec<String> = rows
            .iter()
            .filter(|r| matches!(r.offered.body, Body::Bytes(_)))
            .flat_map(|r| [path(dir, &r.stem, "bin"), path(dir, &r.stem, "json")])
            .filter(|p| p.exists())
            .map(|p| p.display().to_string())
            .collect();
        if !clashes.is_empty() {
            capture::bail!(
                "{} file(s) already exist, refusing to overwrite (pass --force):\n  {}",
                clashes.len(),
                clashes.join("\n  ")
            );
        }
    }

    let os = args
        .os
        .clone()
        .unwrap_or_else(|| backend::os_description().unwrap_or_else(|| backend::NAME.to_owned()));

    let mut written = 0;
    for row in rows {
        let Body::Bytes(bytes) = &row.offered.body else {
            continue;
        };
        std::fs::write(path(dir, &row.stem, "bin"), bytes)?;
        std::fs::write(
            path(dir, &row.stem, "json"),
            build_sidecar(row, bytes.len(), cap, args, &os),
        )?;
        written += 1;
    }
    Ok(written)
}

fn path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    dir.join(format!("{stem}.{ext}"))
}

/// The sidecar `corpus/README.md` asks for, with the captured-fixture fields.
///
/// `app`, `app_version` and `how` are written even when empty, and the empty
/// ones are named on stderr: a blank key that has to be filled in is visible in
/// the diff, whereas a missing key is not.
fn build_sidecar(row: &Row<'_>, len: usize, cap: &Capture, args: &cli::Args, os: &str) -> String {
    let mut fields = vec![
        ("format", Value::str(&row.offered.native)),
        ("flavor", row.flavor.map_or(Value::Null, Value::str)),
        ("origin", Value::str(&args.origin)),
        (
            "description",
            Value::str(format!("{} from {}", row.offered.native, cap.source)),
        ),
        ("expect", Value::str("ok")),
        ("os", Value::str(os)),
        ("app", Value::str(args.app.clone().unwrap_or_default())),
        (
            "app_version",
            Value::str(args.app_version.clone().unwrap_or_default()),
        ),
        ("how", Value::str(args.how.clone().unwrap_or_default())),
        ("bytes", Value::Uint(len as u64)),
    ];
    if let Some(i) = row.offered.item {
        fields.push(("item", Value::Uint(i as u64)));
    }
    // Set by hand after scrubbing, per corpus/README.md — the tool has no way
    // to know whether what it dumped needed redacting.
    fields.push(("redacted", Value::Bool(false)));
    // The backend's note and the operator's note both belong in `notes`; the
    // backend's goes first because it describes how the bytes were obtained,
    // which is context for whatever the operator has to say about them.
    let notes = [row.offered.detail.as_deref(), args.notes.as_deref()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    fields.push(("notes", Value::str(notes)));
    sidecar::object(&fields)
}

// -- reporting ----------------------------------------------------------------

fn report(
    rows: &[Row<'_>],
    cap: &Capture,
    args: &cli::Args,
    written: usize,
) -> capture::Result<()> {
    let err = std::io::stderr();
    let mut out = err.lock();

    if rows.is_empty() {
        writeln!(out, "{} is empty ({})", args.selection.as_str(), cap.source)?;
        return Ok(());
    }

    writeln!(
        out,
        "{} format(s) on the {} ({})",
        rows.len(),
        args.selection.as_str(),
        cap.source
    )?;

    let multi_item = rows.iter().any(|r| r.offered.item.is_some());
    let show_file = args.out.is_some();

    let mut table: Vec<[String; 5]> = vec![[
        "ITEM".into(),
        "FORMAT".into(),
        "FLAVOR".into(),
        "BYTES".into(),
        "FILE".into(),
    ]];
    for row in rows {
        table.push([
            row.offered.item.map(|i| i.to_string()).unwrap_or_default(),
            row.offered.native.clone(),
            row.flavor.unwrap_or("-").to_owned(),
            match &row.offered.body {
                Body::Bytes(b) => b.len().to_string(),
                Body::Skipped(_) => "-".to_owned(),
            },
            match &row.offered.body {
                Body::Bytes(_) if show_file => format!("{}.bin", row.stem),
                Body::Bytes(_) => String::new(),
                Body::Skipped(why) => format!("(skipped: {why})"),
            },
        ]);
    }

    // Columns are dropped rather than left blank so the common case — one
    // item, --list — is three columns wide and readable in a terminal.
    let keep: Vec<usize> = (0..5)
        .filter(|&c| match c {
            0 => multi_item,
            4 => {
                show_file
                    || rows
                        .iter()
                        .any(|r| matches!(r.offered.body, Body::Skipped(_)))
            }
            _ => true,
        })
        .collect();

    let widths: Vec<usize> = keep
        .iter()
        .map(|&c| {
            table
                .iter()
                .map(|r| r[c].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();

    for line in &table {
        let mut s = String::new();
        for (i, &c) in keep.iter().enumerate() {
            let cell = &line[c];
            let pad = widths[i].saturating_sub(cell.chars().count());
            // BYTES right-aligns; everything else reads left to right.
            if c == 3 {
                s.push_str(&" ".repeat(pad));
                s.push_str(cell);
            } else {
                s.push_str(cell);
                if i + 1 < keep.len() {
                    s.push_str(&" ".repeat(pad));
                }
            }
            if i + 1 < keep.len() {
                s.push_str("  ");
            }
        }
        writeln!(out, "  {}", s.trim_end())?;
    }

    if let Some(dir) = &args.out {
        writeln!(out, "wrote {written} .bin + .json pair(s) to {dir}")?;
        let blank: Vec<&str> = [
            ("app", &args.app),
            ("app_version", &args.app_version),
            ("how", &args.how),
        ]
        .iter()
        .filter(|(_, v)| v.is_none())
        .map(|(k, _)| *k)
        .collect();
        if !blank.is_empty() && args.origin == "captured" {
            writeln!(
                out,
                "note: sidecar field(s) left blank: {}. A captured fixture is not \
                 finished until they say enough to repeat the capture.",
                blank.join(", ")
            )?;
        }
        writeln!(
            out,
            "note: scrub usernames, home paths, volume UUIDs and tokens in place and at the \
             same byte length before committing (corpus/README.md)."
        )?;
    }
    Ok(())
}

/// Resolve a native identifier through `rclip-core`'s registry.
///
/// Returns the `Flavor` variant's own name so the table and the sidecar agree,
/// and `None` for [`Flavor::Other`] — printing `Other("x-special/nautilus")`
/// next to the identifier it already contains is noise.
fn flavor_label(native: &str, platform: Platform) -> Option<&'static str> {
    Some(match Flavor::from_native(platform, native) {
        Flavor::PlainText => "PlainText",
        Flavor::Html => "Html",
        Flavor::Rtf => "Rtf",
        Flavor::Png => "Png",
        Flavor::Jpeg => "Jpeg",
        Flavor::Gif => "Gif",
        Flavor::Tiff => "Tiff",
        Flavor::Dib => "Dib",
        Flavor::DibV5 => "DibV5",
        Flavor::FileList => "FileList",
        Flavor::ShellIdList => "ShellIdList",
        Flavor::FileDescriptor => "FileDescriptor",
        Flavor::FileContents => "FileContents",
        Flavor::Url => "Url",
        Flavor::UrlName => "UrlName",
        Flavor::ShellLink => "ShellLink",
        Flavor::DropEffect => "DropEffect",
        // `Flavor` is `#[non_exhaustive]`: a variant added to the registry
        // shows up here as unregistered until this match learns its name.
        _ => return None,
    })
}

//! Walking `corpus/` as one thing.
//!
//! Twelve crates each sweep their own directory, which is twelve chances to
//! forget. This walks the tree from the top, so a fixture in a directory nobody
//! owns, or a sidecar whose `.bin` was deleted, is a failure rather than a file
//! nothing looks at.

use std::path::{Path, PathBuf};

/// The repository's `corpus/` directory.
///
/// Resolved from `CARGO_MANIFEST_DIR` so it works from any working directory,
/// which is what the CI job and `cargo test` disagree about otherwise.
#[must_use]
pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

/// One `.bin` and the `.json` beside it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fixture {
    /// The `.bin`.
    pub bin: PathBuf,
    /// The `.json` sidecar.
    pub sidecar: PathBuf,
    /// Path of the containing directory relative to `corpus/`, slash-separated:
    /// `synthetic/rclip-dropfiles`, `macos/Safari`.
    pub dir: String,
    /// File stem, shared by both files.
    pub stem: String,
}

impl Fixture {
    /// `synthetic/rclip-dropfiles/two-paths-wide`, for messages.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}/{}", self.dir, self.stem)
    }

    /// The last path segment of [`Fixture::dir`]: `rclip-dropfiles`, `Safari`.
    #[must_use]
    pub fn leaf_dir(&self) -> &str {
        self.dir.rsplit('/').next().unwrap_or(&self.dir)
    }

    /// The first path segment of [`Fixture::dir`]: `synthetic`, `macos`.
    #[must_use]
    pub fn top_dir(&self) -> &str {
        self.dir.split('/').next().unwrap_or(&self.dir)
    }
}

/// A file in `corpus/` that is half of a pair, or neither half.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Orphan {
    /// A `.bin` with no `.json` beside it.
    BinWithoutSidecar(PathBuf),
    /// A `.json` with no `.bin` beside it.
    SidecarWithoutBin(PathBuf),
    /// Something that is neither, and is not the corpus README.
    Stray(PathBuf),
}

impl std::fmt::Display for Orphan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BinWithoutSidecar(p) => {
                write!(f, "{}: a .bin with no .json sidecar", p.display())
            }
            Self::SidecarWithoutBin(p) => write!(
                f,
                "{}: a sidecar with no .bin — was the fixture deleted?",
                p.display()
            ),
            Self::Stray(p) => write!(
                f,
                "{}: neither a .bin nor a .json; the corpus holds fixtures and sidecars only",
                p.display()
            ),
        }
    }
}

/// Everything found under `corpus/`.
#[derive(Debug, Default)]
pub struct Corpus {
    /// Complete pairs, sorted by path.
    pub fixtures: Vec<Fixture>,
    /// Everything that is not a complete pair.
    pub orphans: Vec<Orphan>,
    /// Directories that hold at least one fixture, sorted.
    pub dirs: Vec<String>,
}

/// Files that live in the corpus without being fixtures.
fn is_allowed_non_fixture(name: &str) -> bool {
    // A README per directory is documentation, not a fixture. `.DS_Store` and
    // friends are not in the repository and are skipped rather than reported so
    // that a local `cargo test` on a Mac does not fail differently from CI.
    name.eq_ignore_ascii_case("README.md") || name.starts_with('.')
}

/// Walk `corpus/`, recursively, collecting pairs and orphans.
///
/// A missing `corpus/` is an error; an empty one is not, and neither is a
/// directory that appears halfway through a run — the walk is a snapshot.
///
/// # Errors
///
/// Propagates any I/O error other than a directory that vanished mid-walk.
pub fn walk(root: &Path) -> std::io::Result<Corpus> {
    let mut out = Corpus::default();
    let mut queue = vec![root.to_path_buf()];
    let mut dirs = std::collections::BTreeSet::new();

    while let Some(dir) = queue.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            // Another agent is writing corpus/macos/ while this runs; a
            // directory that disappears between listing and reading is not a
            // corpus defect.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && dir != root => continue,
            Err(e) => return Err(e),
        };
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_dir() {
                queue.push(path);
            } else {
                files.push(path);
            }
        }
        files.sort();

        let rel = dir
            .strip_prefix(root)
            .unwrap_or(&dir)
            .to_string_lossy()
            .replace('\\', "/");

        for path in &files {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if is_allowed_non_fixture(&name) {
                continue;
            }
            match path.extension().and_then(|e| e.to_str()) {
                Some("bin") => {
                    let sidecar = path.with_extension("json");
                    if sidecar.is_file() {
                        dirs.insert(rel.clone());
                        out.fixtures.push(Fixture {
                            bin: path.clone(),
                            sidecar,
                            dir: rel.clone(),
                            stem: path
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        });
                    } else {
                        out.orphans.push(Orphan::BinWithoutSidecar(path.clone()));
                    }
                }
                Some("json") => {
                    if !path.with_extension("bin").is_file() {
                        out.orphans.push(Orphan::SidecarWithoutBin(path.clone()));
                    }
                }
                _ => out.orphans.push(Orphan::Stray(path.clone())),
            }
        }
    }

    out.fixtures.sort();
    out.orphans.sort();
    out.dirs = dirs.into_iter().collect();
    Ok(out)
}

//! Argument parsing, hand-rolled.
//!
//! Eight flags do not justify a dependency, and this crate's dependency list
//! should read as "the four OS APIs and nothing else" so that a reader can see
//! at a glance that the tool has no opinions of its own.

use crate::capture::Selection;

pub const USAGE: &str = "\
dump-clipboard — write every format the clipboard is offering to a directory

USAGE:
    dump-clipboard [OPTIONS] [OUTPUT_DIR]

ARGS:
    <OUTPUT_DIR>          Where to write <flavor>.bin and <flavor>.json.
                          Created if missing. Required unless --list.

OPTIONS:
    -l, --list            Print the summary table and write nothing.
        --primary         Read the PRIMARY selection instead of the clipboard.
                          X11 and Wayland only; the other two have no such thing.
    -f, --force           Overwrite files that already exist in OUTPUT_DIR.
        --app <NAME>      Record as the sidecar's `app`.
        --app-version <V> Record as the sidecar's `app_version`.
        --how <TEXT>      Record as the sidecar's `how`: what was copied, and
                          from where, so the capture can be repeated.
        --os <TEXT>       Override the detected OS string.
        --notes <TEXT>    Record as the sidecar's `notes`.
        --origin <WORD>   Sidecar `origin`. Default `captured`.
    -h, --help            Print this.
    -V, --version         Print the version.

Sidecar fields left unset are written as empty strings and reported on stderr:
a captured fixture is not finished until `app`, `app_version` and `how` say
enough to repeat the capture. See corpus/README.md, including its rules on
redacting a capture before it is committed.
";

#[derive(Debug)]
pub struct Args {
    pub out: Option<String>,
    pub list: bool,
    pub selection: Selection,
    pub force: bool,
    pub app: Option<String>,
    pub app_version: Option<String>,
    pub how: Option<String>,
    pub os: Option<String>,
    pub notes: Option<String>,
    pub origin: String,
}

/// What `main` should do next.
#[derive(Debug)]
pub enum Parsed {
    Run(Box<Args>),
    /// Print this on stdout and exit 0.
    Print(String),
}

pub fn parse<I: Iterator<Item = String>>(argv: I) -> Result<Parsed, String> {
    let mut a = Args {
        out: None,
        list: false,
        selection: Selection::Clipboard,
        force: false,
        app: None,
        app_version: None,
        how: None,
        os: None,
        notes: None,
        origin: "captured".to_owned(),
    };

    let mut it = argv.peekable();
    // `value` is a closure over the iterator, so every option that takes an
    // argument reports the same way when it is missing one.
    macro_rules! value {
        ($flag:expr) => {
            match it.next() {
                Some(v) => v,
                None => return Err(format!("{} needs a value", $flag)),
            }
        };
    }

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Print(USAGE.to_owned())),
            "-V" | "--version" => {
                return Ok(Parsed::Print(format!(
                    "dump-clipboard {}\n",
                    env!("CARGO_PKG_VERSION")
                )))
            }
            "-l" | "--list" => a.list = true,
            "-f" | "--force" => a.force = true,
            "--primary" => a.selection = Selection::Primary,
            "--clipboard" => a.selection = Selection::Clipboard,
            "-o" | "--out" => a.out = Some(value!("--out")),
            "--app" => a.app = Some(value!("--app")),
            "--app-version" => a.app_version = Some(value!("--app-version")),
            "--how" => a.how = Some(value!("--how")),
            "--os" => a.os = Some(value!("--os")),
            "--notes" => a.notes = Some(value!("--notes")),
            "--origin" => a.origin = value!("--origin"),
            "--" => {
                if let Some(v) = it.next() {
                    set_positional(&mut a, v)?;
                }
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("unknown option `{other}` (try --help)"))
            }
            other => set_positional(&mut a, other.to_owned())?,
        }
    }

    if a.out.is_none() && !a.list {
        return Err("an output directory is required (or pass --list)".to_owned());
    }
    Ok(Parsed::Run(Box::new(a)))
}

fn set_positional(a: &mut Args, v: String) -> Result<(), String> {
    if a.out.is_some() {
        return Err(format!("unexpected second output directory `{v}`"));
    }
    a.out = Some(v);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Result<Args, String> {
        match parse(s.iter().map(|s| (*s).to_owned())) {
            Ok(Parsed::Run(a)) => Ok(*a),
            Ok(Parsed::Print(_)) => Err("printed".to_owned()),
            Err(e) => Err(e),
        }
    }

    #[test]
    fn positional_output_directory() {
        let a = args(&["corpus/macos/textedit"]).unwrap();
        assert_eq!(a.out.as_deref(), Some("corpus/macos/textedit"));
        assert_eq!(a.selection, Selection::Clipboard);
        assert!(!a.list);
    }

    #[test]
    fn list_needs_no_directory() {
        assert!(args(&["--list"]).unwrap().out.is_none());
        assert!(args(&[]).is_err());
    }

    #[test]
    fn options_with_values() {
        let a = args(&["-l", "--app", "Safari", "--how", "copied a table"]).unwrap();
        assert_eq!(a.app.as_deref(), Some("Safari"));
        assert_eq!(a.how.as_deref(), Some("copied a table"));
    }

    #[test]
    fn a_missing_value_is_an_error_not_a_silent_default() {
        assert!(args(&["-l", "--app"]).is_err());
    }

    #[test]
    fn unknown_options_are_rejected() {
        assert!(args(&["--nope"]).is_err());
        // A directory that begins with a dash still has to be reachable.
        assert_eq!(
            args(&["--", "-weird"]).unwrap().out.as_deref(),
            Some("-weird")
        );
    }
}

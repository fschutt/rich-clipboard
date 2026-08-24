//! Dump every format the system clipboard is currently offering.
//!
//! This is how the captured half of the corpus gets made, and it is the tool
//! you reach for whenever a paste misbehaves: it answers "what did that
//! application actually put on the clipboard" without guessing.
//!
//! Per-platform backends are filled in by the transport work; this shell is
//! here so the workspace has a home for them.

fn main() {
    eprintln!("dump-clipboard: no backend compiled in for this platform yet");
    std::process::exit(2);
}

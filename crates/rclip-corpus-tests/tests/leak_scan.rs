//! Deliverable B: nothing in `corpus/` identifies a machine or a person.
//!
//! A capture is cut from a real machine. The one captured bookmark in this
//! corpus arrived carrying a boot-volume UUID and a sandbox HMAC, and it was
//! caught by a human reading a report. This is the check that was missing.
//!
//! Both halves of a fixture are scanned. A sidecar whose `notes` quote the
//! original value in prose — "the volume UUID was 1D4F…" — leaks it exactly as
//! effectively as the bytes did, and is the easier mistake to make, because the
//! bytes are the part you remember to scrub.

use rclip_corpus_tests::scan::{self, Identity, Rule};
use rclip_corpus_tests::{corpus_root, report, sidecar, walk};

fn identity() -> Identity {
    let id = scan::identity_from_env();
    assert!(
        id.login.is_some() || id.home.is_some(),
        "neither USER/LOGNAME/USERNAME nor HOME is set, so the scan cannot know \
         whose identity to look for; set one before running this"
    );
    id
}

#[test]
fn nothing_in_the_corpus_identifies_a_machine_or_a_person() {
    let id = identity();
    let root = corpus_root();
    let found = walk(&root).expect("corpus/ must exist");
    let mut problems = Vec::new();
    let mut scanned = 0usize;
    let mut allowlisted = 0usize;

    for f in &found.fixtures {
        let text = std::fs::read_to_string(&f.sidecar).unwrap_or_default();
        let parsed = sidecar::parse(&text).ok();
        let allowed: Vec<String> = parsed
            .as_ref()
            .map(|s| s.leak_allow.clone())
            .unwrap_or_default();
        let allows = |r: Rule| allowed.iter().any(|a| a == r.name() || a == "*");

        // --- the bytes ----------------------------------------------------
        let bytes = std::fs::read(&f.bin).expect("fixture");
        scanned += 1;
        for finding in scan::scan(&bytes, &id) {
            if allows(finding.rule) {
                allowlisted += 1;
                continue;
            }
            problems.push(format!("{}.bin  {finding}", f.label()));
        }

        // --- the prose ----------------------------------------------------
        //
        // Scanned per value rather than over the raw file, so `\uXXXX` escapes
        // are decoded first and the message can name the key.
        let Some(s) = parsed else { continue };
        for (key, value) in &s.raw {
            // An `expect_paths` array is prose too, and it is the one place a
            // pinned expectation spells out a real path.
            let texts: Vec<(String, &str)> = match value {
                v if v.as_str().is_some() => vec![(key.clone(), v.as_str().unwrap())],
                v if v.as_array().is_some() => v
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .filter_map(|(i, item)| Some((format!("{key}[{i}]"), item.as_str()?)))
                    .collect(),
                _ => continue,
            };
            for (where_, text) in texts {
                for finding in scan::scan_text(text, &id) {
                    if allows(finding.rule) {
                        allowlisted += 1;
                        continue;
                    }
                    problems.push(format!("{}.json  \"{where_}\"  {finding}", f.label()));
                }
            }
        }
    }

    report(
        "corpus content that must not be in a public repository\n\
         (if a match is a false positive, add \"leak_allow\" and \"leak_allow_reason\" \
         to the sidecar)",
        &problems,
    );
    assert_eq!(
        scanned,
        found.fixtures.len(),
        "every fixture the walker found has to be scanned"
    );
    assert!(scanned >= 100, "corpus/ has shrunk to {scanned} fixtures");
    // Not an assertion about the number, just a note in the log for whoever is
    // reading a CI run.
    println!("scanned {scanned} fixtures, {allowlisted} allowlisted matches");
}

/// A redaction has to keep the byte length, or the fixture stops being a
/// faithful capture of the layout — which is the only reason a capture is worth
/// more than hand-built bytes. The placeholders are therefore fixed-width, and
/// this checks that a `redacted` sidecar actually says what was replaced.
#[test]
fn a_redacted_fixture_says_what_was_replaced() {
    let root = corpus_root();
    let found = walk(&root).expect("corpus/ must exist");
    let mut problems = Vec::new();
    for f in &found.fixtures {
        let text = std::fs::read_to_string(&f.sidecar).unwrap_or_default();
        let Ok(s) = sidecar::parse(&text) else {
            continue;
        };
        if !s.redacted {
            continue;
        }
        let notes = s.notes.unwrap_or_default();
        if !notes.to_lowercase().contains("redact") && !notes.to_lowercase().contains("replaced") {
            problems.push(format!(
                "{}: \"redacted\": true, but the notes never say which fields were replaced \
                 or with what. A redaction nobody documented is one nobody can re-take.",
                f.label()
            ));
        }
    }
    report("redactions with no record of what was redacted", &problems);
}

/// The scanner has to be able to fail. If a rule silently stopped matching —
/// a refactor, a bad regex-free matcher, an over-eager allowlist — every run
/// would be green and the corpus would be unguarded. This feeds it a fixture
/// built to trip every rule and insists it does.
#[test]
fn the_scanner_catches_a_deliberately_leaky_payload() {
    let id = Identity {
        login: Some("acontributor".into()),
        home: Some("/Users/acontributor".into()),
        home_base: Some("acontributor".into()),
    };

    let cases: &[(&[u8], Rule)] = &[
        (b"/Users/acontributor/Desktop/x", Rule::CurrentUser),
        (b"/Users/jbloggs/Desktop/x", Rule::HomePath),
        (b"vol 6A5C3D1E-7B42-4F98-9C11-A0D2E3F45B67", Rule::Uuid),
        (b"tok 9f8e7d6c5b4a39281706f5e4d3c2b1a0ff", Rule::HexRun),
        (b"from j.bloggs@somewhere.co.uk", Rule::Email),
        (
            "Bartholomew\u{2019}s MacBook Pro".as_bytes(),
            Rule::PersonalDevice,
        ),
    ];

    let mut missed = Vec::new();
    for (payload, want) in cases {
        let hits = scan::scan(payload, &id);
        if !hits.iter().any(|f| f.rule == *want) {
            missed.push(format!(
                "{want} did not fire on {:?}",
                String::from_utf8_lossy(payload)
            ));
        }
    }
    // The same payloads again, this time as UTF-16LE, since half of these
    // formats store their strings that way and a scanner that only reads ASCII
    // is a scanner that misses every Windows capture.
    for (payload, want) in cases {
        let wide: Vec<u8> = String::from_utf8_lossy(payload)
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let hits = scan::scan(&wide, &id);
        if !hits.iter().any(|f| f.rule == *want) {
            missed.push(format!("{want} did not fire on the UTF-16LE spelling"));
        }
    }
    report("leak-scanner rules that no longer fire", &missed);
}

/// Every rule a sidecar allowlists has to be a rule that exists. A typo in
/// `leak_allow` silences nothing and looks like it silences something, which is
/// the worst of both.
#[test]
fn allowlist_entries_name_real_rules() {
    let root = corpus_root();
    let found = walk(&root).expect("corpus/ must exist");
    let names: Vec<&str> = Rule::all().iter().map(|r| r.name()).collect();
    let mut problems = Vec::new();
    for f in &found.fixtures {
        let text = std::fs::read_to_string(&f.sidecar).unwrap_or_default();
        let Ok(s) = sidecar::parse(&text) else {
            continue;
        };
        for entry in &s.leak_allow {
            if entry != "*" && !names.contains(&entry.as_str()) {
                problems.push(format!(
                    "{}: \"leak_allow\" names {entry:?}, which is not a rule. Rules are: {}",
                    f.label(),
                    names.join(", ")
                ));
            }
        }
    }
    report("allowlist entries that silence nothing", &problems);
}

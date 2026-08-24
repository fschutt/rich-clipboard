//! The asymmetry in `SizeHint` is the whole point of the type, so it is what
//! these tests hammer: a lower bound can prove a payload too big and never
//! prove it small enough, and "unknown" proves neither.

use rclip_core::{Budget, Flavor, Limits, Oversize, OversizePolicy, SizeHint};

#[test]
fn an_exact_hint_decides_both_ways() {
    let l = Limits::default();
    assert!(SizeHint::Exact(l.max_flavor_bytes + 1).definitely_exceeds(l.max_flavor_bytes));
    assert!(!SizeHint::Exact(l.max_flavor_bytes).definitely_exceeds(l.max_flavor_bytes));
    assert_eq!(SizeHint::Exact(42).known_bytes(), Some(42));
}

#[test]
fn a_lower_bound_can_reject_but_never_admit() {
    // X11's INCR gives a floor. A floor over the cap is proof of oversize.
    assert!(SizeHint::AtLeast(1 << 30).definitely_exceeds(1 << 20));
    // But a floor under the cap proves nothing: the real payload may be
    // larger. The value is still reported, so a caller can use it as a hint —
    // it just must not treat it as an all-clear.
    assert!(!SizeHint::AtLeast(1).definitely_exceeds(1 << 20));
    assert_eq!(SizeHint::AtLeast(1).known_bytes(), Some(1));
}

#[test]
fn unknown_is_not_zero_and_not_small() {
    // The Wayland case. Treating "no information" as "small" is exactly the
    // mistake that makes an unbounded pipe read the way in, so `known_bytes`
    // returns None rather than 0 and `definitely_exceeds` stays false.
    assert_eq!(SizeHint::Unknown.known_bytes(), None);
    assert!(!SizeHint::Unknown.definitely_exceeds(0));
    assert!(!Limits::default().rejects(SizeHint::Unknown));
}

#[test]
fn the_default_policy_skips_rather_than_aborts() {
    // Dropping one oversize flavor still leaves the rest of the paste usable:
    // a 400 MB TIFF goes, the plain text stays.
    struct Silent;
    impl OversizePolicy for Silent {}
    let mut p = Silent;
    assert_eq!(
        p.on_oversize(Flavor::Tiff, SizeHint::Exact(1 << 30), &Limits::default()),
        Oversize::Skip
    );
}

#[test]
fn a_closure_is_a_policy() {
    let mut seen = None;
    {
        let mut policy = |f: Flavor<'_>, h: SizeHint, _: &Limits| {
            seen = Some((format!("{f:?}"), h));
            Oversize::Abort
        };
        let got = policy.on_oversize(Flavor::Png, SizeHint::AtLeast(999), &Limits::default());
        assert_eq!(got, Oversize::Abort);
    }
    assert_eq!(seen.unwrap().1, SizeHint::AtLeast(999));
}

#[test]
fn unlimited_really_is_unlimited() {
    let l = Limits::UNLIMITED;
    assert!(!l.rejects(SizeHint::Exact(u64::MAX)));
    assert!(!l.rejects(SizeHint::AtLeast(u64::MAX)));
}

#[test]
fn budget_stops_exactly_at_the_limit() {
    let mut b = Budget::new(10);
    assert!(b.consume(4));
    assert!(
        b.consume(6),
        "consuming exactly the limit is still within it"
    );
    assert!(b.is_exhausted());
    assert!(!b.consume(1), "one byte past must fail");
    assert_eq!(b.spent(), 11, "spent counts what was offered, not what fit");
}

#[test]
fn budget_does_not_overflow_on_a_hostile_chunk() {
    // A pipe read that claims u64::MAX must clamp, not wrap. This is the
    // Wayland path, where the length is whatever the sender says it is.
    let mut b = Budget::new(1024);
    assert!(!b.consume(u64::MAX));
    assert_eq!(b.remaining(), 0);
    assert!(b.is_exhausted());
    // And it stays exhausted rather than wrapping back to plenty.
    assert!(!b.consume(1));
    assert_eq!(b.remaining(), 0);
}

#[test]
fn defaults_admit_a_realistic_paste_and_reject_a_hostile_one() {
    let l = Limits::default();
    // A 12-megapixel screenshot as RGBA is ~48 MB. Must pass.
    assert!(!l.rejects(SizeHint::Exact(48 * 1024 * 1024)));
    // A gigabyte must not.
    assert!(l.rejects(SizeHint::Exact(1024 * 1024 * 1024)));
    // The aggregate cap is looser than the per-flavor one, because a real
    // source offers the same content many ways — a Safari copy offers eleven.
    assert!(l.max_total_bytes > l.max_flavor_bytes);
}

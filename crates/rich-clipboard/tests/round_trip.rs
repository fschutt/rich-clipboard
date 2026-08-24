//! Round trips through the `RichText` hub.
//!
//! The hub's promise is that N formats cost N conversions rather than N².
//! These tests pin what survives a trip through it, and — just as usefully —
//! what does not.

#![cfg(feature = "rtf")]

use rich_clipboard::{Rgb, RichText, Style};

fn sample() -> RichText {
    let mut text = RichText::default();
    text.push("plain ", Style::default());
    text.push(
        "bold",
        Style {
            bold: true,
            ..Style::default()
        },
    );
    text.push(" and ", Style::default());
    text.push(
        "italic red",
        Style {
            italic: true,
            underline: true,
            strikethrough: true,
            color: Some(Rgb::new(255, 0, 0)),
            background: Some(Rgb::new(0, 0, 255)),
            font_family: Some("Courier New".into()),
            size_pt: Some(14.0),
            ..Style::default()
        },
    );
    text.push("\nsecond line", Style::default());
    text
}

#[test]
fn every_style_property_survives_a_trip_through_rtf() {
    let before = sample();
    let after = RichText::from_rtf(&before.to_rtf()).unwrap();

    assert_eq!(after.as_str(), before.as_str());

    let a: Vec<_> = after.spans().collect();
    let b: Vec<_> = before.spans().collect();
    assert_eq!(a.len(), b.len(), "runs did not survive: {a:#?}");
    for (i, ((at, astyle), (bt, bstyle))) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(at, bt, "run {i} text");
        assert_eq!(astyle.bold, bstyle.bold, "run {i} bold");
        assert_eq!(astyle.italic, bstyle.italic, "run {i} italic");
        assert_eq!(astyle.underline, bstyle.underline, "run {i} underline");
        assert_eq!(
            astyle.strikethrough, bstyle.strikethrough,
            "run {i} strikethrough"
        );
        assert_eq!(astyle.color, bstyle.color, "run {i} colour");
        assert_eq!(astyle.background, bstyle.background, "run {i} background");
        assert_eq!(
            astyle.font_family, bstyle.font_family,
            "run {i} font family"
        );
    }
}

#[test]
fn an_unstated_size_comes_back_stated_because_rtf_has_no_way_to_leave_it_out() {
    // The documented asymmetry of the hub: `Style::size_pt` is `None` for
    // "inherit", and RTF has no spelling for that — `\fsN` is always present
    // and its default is 24 half-points. A round trip through RTF therefore
    // turns "inherit" into "12pt", which is the right *rendering* and a
    // different *statement*.
    let before = RichText::plain("hello");
    assert_eq!(before.runs[0].style.size_pt, None);

    let after = RichText::from_rtf(&before.to_rtf()).unwrap();
    assert_eq!(after.as_str(), "hello");
    assert_eq!(after.runs[0].style.size_pt, Some(12.0));
}

#[test]
fn non_ascii_survives_as_escapes_and_never_as_raw_high_bytes() {
    // The reader on the other end may be running under a different `\ansicpg`
    // than we assumed, so a raw high byte would arrive as a *different*
    // character rather than as a visible gap.
    let before = RichText::plain("café — wörld 😀 日本語");
    let rtf = before.to_rtf();

    assert!(
        rtf.iter().all(u8::is_ascii),
        "the writer emitted a raw high byte"
    );
    assert!(rtf.windows(2).any(|w| w == br"\u"));

    let after = RichText::from_rtf(&rtf).unwrap();
    assert_eq!(after.as_str(), before.as_str());
}

#[test]
fn the_rtf_metacharacters_are_escaped_rather_than_taken_as_syntax() {
    let before = RichText::plain(r"a { b } c \ d");
    let after = RichText::from_rtf(&before.to_rtf()).unwrap();
    assert_eq!(after.as_str(), r"a { b } c \ d");
}

#[test]
fn a_tab_and_a_paragraph_break_survive() {
    let before = RichText::plain("one\ttwo\nthree");
    let after = RichText::from_rtf(&before.to_rtf()).unwrap();
    assert_eq!(after.as_str(), "one\ttwo\nthree");
}

#[test]
fn adjacent_runs_with_the_same_style_are_merged() {
    // `rclip-rtf` cuts a run at every group boundary and every escape, so an
    // unmerged document can be one run per character. Without merging,
    // re-serializing would emit a full property reset between each of them.
    let mut text = RichText::default();
    for _ in 0..10 {
        text.push("x", Style::default());
    }
    assert_eq!(text.runs.len(), 1);

    let after = RichText::from_rtf(&text.to_rtf()).unwrap();
    assert_eq!(after.runs.len(), 1);
    assert_eq!(after.as_str(), "xxxxxxxxxx");
}

#[test]
fn an_empty_document_round_trips_to_an_empty_document() {
    let after = RichText::from_rtf(&RichText::default().to_rtf()).unwrap();
    assert!(after.is_empty());
    assert!(after.runs.is_empty());
}

#[cfg(feature = "html")]
#[test]
fn the_html_leg_writes_inline_css_and_escapes_its_text() {
    let mut text = RichText::default();
    text.push("a < b & \"c\"", Style::default());
    text.push(
        "styled",
        Style {
            bold: true,
            italic: true,
            underline: true,
            strikethrough: true,
            color: Some(Rgb::new(0x12, 0x34, 0x56)),
            ..Style::default()
        },
    );

    let html = text.to_html_fragment();
    // The unstyled run is bare text: no wrapper, because there is nothing to
    // say about it.
    assert!(
        html.starts_with("a &lt; b &amp; &quot;c&quot;<span "),
        "{html}"
    );
    assert!(html.contains("font-weight:700"));
    assert!(html.contains("font-style:italic"));
    // One declaration: a second `text-decoration` would override the first
    // rather than adding to it.
    assert!(html.contains("text-decoration:underline line-through"));
    assert!(html.contains("color:#123456"));
    assert_eq!(html.matches("<span ").count(), 1);
}

#[cfg(feature = "html")]
#[test]
fn cf_html_offsets_survive_a_round_trip_through_the_builder() {
    let text = sample();
    let blob = text.to_cf_html(Some("https://example.com/a")).unwrap();
    let parsed = rclip_cf_html::parse(&blob).unwrap();
    assert_eq!(parsed.fragment, text.to_html_fragment());
    assert_eq!(parsed.source_url, Some("https://example.com/a"));
}

#[cfg(feature = "html")]
#[test]
fn a_newline_becomes_a_break_in_html_and_a_paragraph_in_rtf() {
    // The one place the two legs of the hub genuinely disagree, so it is worth
    // pinning rather than discovering.
    let text = RichText::plain("a\nb");
    assert_eq!(text.to_html_fragment(), "a<br>b");
    assert!(text.to_rtf().windows(5).any(|w| w == br"\par "));
}

#[test]
fn a_round_trip_through_a_payload_keeps_the_styling() {
    // The whole loop a consumer runs: publish, hand to a transport, read back.
    use rclip_core::Platform;
    use rich_clipboard::{decode_payload, encode, RichItem};

    for platform in [Platform::Windows, Platform::MacOs] {
        let payload = encode(&RichItem::RichText(sample()), platform).unwrap();
        match decode_payload(&payload).unwrap() {
            RichItem::RichText(back) => {
                assert_eq!(back.as_str(), sample().as_str(), "on {platform:?}");
                assert!(back.runs[1].style.bold, "on {platform:?}");
            }
            other => panic!("on {platform:?}, expected styled text, got {other:?}"),
        }
    }
}

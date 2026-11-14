// SPDX-License-Identifier: MPL-2.0

mod common;

use capsa_core::census_snapshot::{Source, Window, read_since_from};
use census::statusline::{line, parse, record, snapshots};
use common::Scratch;

const LIMITS: &str = r#"{"rate_limits":{"five_hour":{"used_percentage":6,"resets_at":1793530800},"seven_day":{"used_percentage":1,"resets_at":1794106800}}}"#;

#[test]
fn each_present_window_gives_one_statusline_snapshot() {
    let snaps = snapshots(&parse(LIMITS).expect("the sample parses"), 100);
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[0].window, Window::FiveHour);
    assert!((snaps[0].used_percentage - 6.0).abs() < 1e-9);
    assert_eq!(snaps[0].resets_at, 1_793_530_800);
    assert_eq!(snaps[1].window, Window::SevenDay);
    assert!(
        snaps
            .iter()
            .all(|s| s.t == 100 && s.source == Source::Statusline)
    );
}

#[test]
fn absent_windows_and_fields_are_skipped() {
    let only_five =
        parse(r#"{"rate_limits":{"five_hour":{"used_percentage":6}}}"#).expect("parses");
    assert!(snapshots(&only_five, 1).is_empty());
    let only_seven = parse(r#"{"rate_limits":{"seven_day":{"used_percentage":1,"resets_at":5}}}"#)
        .expect("parses");
    let snaps = snapshots(&only_seven, 1);
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].window, Window::SevenDay);
    assert!(snapshots(&parse("{}").expect("parses"), 1).is_empty());
}

#[test]
fn record_skips_a_reading_identical_to_the_last_one_of_its_window() {
    let scratch = Scratch::new("record");
    let file = scratch.path().join("snapshots");
    let first = snapshots(&parse(LIMITS).expect("parses"), 100);
    assert_eq!(record(&file, &first).expect("written"), 2);
    let again = snapshots(&parse(LIMITS).expect("parses"), 200);
    assert_eq!(record(&file, &again).expect("checked"), 0);
    let moved =
        parse(&LIMITS.replace("\"used_percentage\":6", "\"used_percentage\":7")).expect("parses");
    assert_eq!(record(&file, &snapshots(&moved, 300)).expect("written"), 1);
    assert_eq!(read_since_from(&file, 0).expect("read").len(), 3);
}

#[test]
fn record_compares_with_the_tail_of_a_long_file() {
    let scratch = Scratch::new("tail");
    let snaps = snapshots(&parse(LIMITS).expect("parses"), 100);
    let mut body = "x".repeat(20_000);
    body.push('\n');
    for snap in &snaps {
        body.push_str(&serde_json::to_string(snap).expect("encodes"));
        body.push('\n');
    }
    let file = scratch.write("snapshots", &body);
    assert_eq!(record(&file, &snaps).expect("checked"), 0);
}

#[test]
fn line_lists_the_present_windows() {
    let input = parse(LIMITS).expect("parses");
    let snaps = snapshots(&input, 100);
    assert_eq!(line(&input, &snaps, Vec::new()), "5h 6% · wk 1%");
}

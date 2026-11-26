// SPDX-License-Identifier: MPL-2.0

mod common;

use capsa_core::census_snapshot::{Source, Window, read_since_from};
use census::statusline::{line, parse, record, snapshots};
use common::Scratch;

const LIMITS: &str = r#"{"rate_limits":{"five_hour":{"used_percentage":6,"resets_at":1793530800},"seven_day":{"used_percentage":1,"resets_at":1794106800}}}"#;

#[test]
fn each_complete_window_gives_one_snapshot_and_incomplete_ones_are_skipped() {
    type Case = (&'static str, Vec<(Window, f64, u64)>);
    let cases: [Case; 4] = [
        (
            LIMITS,
            vec![
                (Window::FiveHour, 6.0, 1_793_530_800),
                (Window::SevenDay, 1.0, 1_794_106_800),
            ],
        ),
        (
            r#"{"rate_limits":{"five_hour":{"used_percentage":6}}}"#,
            vec![],
        ),
        (
            r#"{"rate_limits":{"seven_day":{"used_percentage":1,"resets_at":5}}}"#,
            vec![(Window::SevenDay, 1.0, 5)],
        ),
        ("{}", vec![]),
    ];
    for (input, expected) in cases {
        let snaps = snapshots(&parse(input).expect("parses"), 100);
        let got: Vec<(Window, f64, u64)> = snaps
            .iter()
            .map(|s| (s.window, s.used_percentage, s.resets_at))
            .collect();
        assert_eq!(got, expected, "{input}");
        assert!(
            snaps
                .iter()
                .all(|s| s.t == 100 && s.source == Source::Statusline),
            "{input}"
        );
    }
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

// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn statusline_records_snapshots_once_per_change() {
    let home = Scratch::new("cli-statusline");
    let cwd = home.path().to_path_buf();
    home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
    let out = census(home.path(), &cwd, &["statusline", "--line"], STATUS, &[]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout), "ctx 44k/400k · 5h 6% · wk 1%\n");
    let file = home.path().join(if cfg!(windows) {
        "state/census/state/snapshots"
    } else {
        "state/census/snapshots"
    });
    assert_eq!(
        std::fs::read_to_string(&file)
            .expect("snapshots")
            .lines()
            .count(),
        2
    );

    census(home.path(), &cwd, &["statusline"], STATUS, &[]);
    assert_eq!(
        std::fs::read_to_string(&file)
            .expect("snapshots")
            .lines()
            .count(),
        2
    );
}

#[test]
fn statusline_is_silent_by_default_and_exits_zero_on_bad_input() {
    let home = Scratch::new("cli-silent");
    let cwd = home.path().to_path_buf();
    let quiet = census(home.path(), &cwd, &["statusline"], STATUS, &[]);
    assert_eq!(quiet.status.code(), Some(0));
    assert!(quiet.stdout.is_empty());
    let bad = census(
        home.path(),
        &cwd,
        &["statusline", "--line"],
        "not json",
        &[],
    );
    assert_eq!(bad.status.code(), Some(0));
    assert!(bad.stdout.is_empty());
}

#[test]
fn statusline_still_prints_the_line_when_the_state_dir_is_unwritable() {
    let home = Scratch::new("cli-unwritable");
    let cwd = home.path().to_path_buf();
    home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
    home.write("state", "a file, not a directory");
    let out = census(home.path(), &cwd, &["statusline", "--line"], STATUS, &[]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout), "ctx 44k/400k · 5h 6% · wk 1%\n");
}

const NOW: u64 = 1_791_964_800;
const BASE: &str = "ctx 44k/400k · 5h 6% · wk 1%\n";

fn bulletin_path(tool: &str) -> String {
    if cfg!(windows) {
        format!("state/capsa/state/bulletin/{tool}.json")
    } else {
        format!("state/capsa/bulletin/{tool}.json")
    }
}

fn write_bulletin(home: &Scratch, tool: &str, value: &Value) {
    home.write(&bulletin_path(tool), &value.to_string());
}

/// Runs `statusline --line` at `NOW` with a 400k window, as `session`.
fn line_as(home: &Scratch, session: &str) -> String {
    home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
    let input = STATUS.replace(
        r#""session_id":"s1""#,
        &format!(r#""session_id":"{session}""#),
    );
    let now = NOW.to_string();
    let out = census(
        home.path(),
        home.path(),
        &["statusline", "--line"],
        &input,
        &[("CENSUS_NOW", now.as_str())],
    );
    assert_eq!(out.status.code(), Some(0));
    text(&out.stdout)
}

#[test]
fn deepseek_gate_shows_the_balance_when_open_and_off_when_closed() {
    let home = Scratch::new("cli-statusline-deepseek");
    let gate = |open: bool, balance: Value| {
        json!({"deepseek": {"at": NOW, "ttl": 600,
            "value": {"open": open, "balance_usd": balance}}})
    };
    write_bulletin(&home, "vicarius", &gate(true, json!(8.66)));
    assert_eq!(
        line_as(&home, "s1"),
        "ctx 44k/400k · 5h 6% · wk 1% · ds $8.66\n"
    );
    write_bulletin(&home, "vicarius", &gate(true, Value::Null));
    assert_eq!(
        line_as(&home, "s1"),
        "ctx 44k/400k · 5h 6% · wk 1% · ds open\n"
    );
    write_bulletin(&home, "vicarius", &gate(false, json!(8.66)));
    assert_eq!(
        line_as(&home, "s1"),
        "ctx 44k/400k · 5h 6% · wk 1% · ds off\n"
    );
}

#[test]
fn deepseek_segment_is_absent_when_the_bulletin_is_missing_stale_or_unreadable() {
    let home = Scratch::new("cli-statusline-deepseek-absent");
    assert_eq!(line_as(&home, "s1"), BASE);
    write_bulletin(
        &home,
        "vicarius",
        &json!({"deepseek": {"at": NOW - 601, "ttl": 600,
            "value": {"open": true, "balance_usd": 8.66}}}),
    );
    assert_eq!(line_as(&home, "s1"), BASE);
    home.write(&bulletin_path("vicarius"), "not json");
    assert_eq!(line_as(&home, "s1"), BASE);
}

#[test]
fn guard_blocks_show_for_their_session_only_and_only_when_positive() {
    let home = Scratch::new("cli-statusline-guard");
    write_bulletin(
        &home,
        "acus",
        &json!({"guard_blocks": {"at": NOW, "ttl": 600, "value": {"s1": 3, "s3": 0}}}),
    );
    assert_eq!(
        line_as(&home, "s1"),
        "ctx 44k/400k · 5h 6% · wk 1% · guard 3\n"
    );
    assert_eq!(line_as(&home, "s2"), BASE);
    assert_eq!(line_as(&home, "s3"), BASE);
    write_bulletin(
        &home,
        "vicarius",
        &json!({"deepseek": {"at": NOW, "ttl": 600,
            "value": {"open": true, "balance_usd": 8.66}}}),
    );
    assert_eq!(
        line_as(&home, "s1"),
        "ctx 44k/400k · 5h 6% · wk 1% · ds $8.66 · guard 3\n"
    );
    write_bulletin(
        &home,
        "acus",
        &json!({"guard_blocks": {"at": NOW - 601, "ttl": 600, "value": {"s1": 3}}}),
    );
    assert_eq!(
        line_as(&home, "s1"),
        "ctx 44k/400k · 5h 6% · wk 1% · ds $8.66\n"
    );
}

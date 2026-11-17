// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn set_thresholds_store_sorted_unique_lists_and_show_them() {
    let home = Scratch::new("cli-thresholds-set");
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &[]);

    let five = run(&[
        "--json",
        "self",
        "config",
        "set",
        "five-hour-thresholds",
        "95,85,85",
    ]);
    assert_eq!(five.status.code(), Some(0), "{}", text(&five.stderr));
    let json: Value = serde_json::from_str(&text(&five.stdout)).expect("json");
    assert_eq!(json["five_hour_thresholds"], json!([85, 95]));

    let weekly = run(&[
        "--json",
        "self",
        "config",
        "set",
        "weekly-thresholds",
        "99, 70",
    ]);
    assert_eq!(weekly.status.code(), Some(0), "{}", text(&weekly.stderr));
    let json: Value = serde_json::from_str(&text(&weekly.stdout)).expect("json");
    assert_eq!(json["weekly_thresholds"], json!([70, 99]));

    let daily = run(&[
        "--json",
        "self",
        "config",
        "set",
        "daily-thresholds",
        "150,50,50",
    ]);
    assert_eq!(daily.status.code(), Some(0), "{}", text(&daily.stderr));
    let json: Value = serde_json::from_str(&text(&daily.stdout)).expect("json");
    assert_eq!(json["daily_thresholds"], json!([50, 150]));

    let shown = text(&run(&["self", "config", "show"]).stdout);
    assert!(shown.contains("5h thresholds"), "{shown}");
    assert!(shown.contains("85 %, 95 %"), "{shown}");
    assert!(shown.contains("70 %, 99 %"), "{shown}");
    assert!(shown.contains("50 %, 150 %"), "{shown}");
}

#[test]
fn set_thresholds_refuse_an_empty_or_invalid_list_with_a_hint() {
    let home = Scratch::new("cli-thresholds-bad");
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &[]);
    for args in [
        ["self", "config", "set", "five-hour-thresholds", ""],
        ["self", "config", "set", "five-hour-thresholds", "85,,95"],
        ["self", "config", "set", "five-hour-thresholds", "0"],
        ["self", "config", "set", "weekly-thresholds", "101"],
        ["self", "config", "set", "weekly-thresholds", "ninety"],
        ["self", "config", "set", "daily-thresholds", "50,0"],
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("hint:"), "{}", text(&out.stderr));
    }
    assert!(!home.path().join(CONFIG_FILE).exists());
}

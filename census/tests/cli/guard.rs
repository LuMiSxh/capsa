// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn guard_log_counts_rotated_and_current_lines_and_skips_bad_ones() {
    let home = Scratch::new("cli-guard");
    let cwd = home.path().to_path_buf();
    home.write(
        "state/acus/guard.jsonl.old",
        &jsonl(&[json!({"ts": 10, "rule": "grep", "kind": "deny", "call": "grep a"})]),
    );
    home.write(
        "state/acus/guard.jsonl",
        &format!(
            "{}not json\n{}",
            jsonl(&[json!({"ts": 20, "rule": "grep", "kind": "hint", "call": "grep b"})]),
            jsonl(&[json!({"ts": 30, "rule": "sed", "kind": "escape", "call": "sed -i x"})]),
        ),
    );
    let out = census(home.path(), &cwd, &["usage", "--guard", "--json"], "", &[]);
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    let rules = v["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["deny"], 1);
    assert_eq!(rules[0]["hint"], 1);
    assert_eq!(rules[1]["escape"], 1);
    assert_eq!(v["escaped"].as_array().map(Vec::len), Some(1));
}

#[test]
fn guard_log_reads_the_current_line_format() {
    let home = Scratch::new("cli-guard-current");
    let cwd = home.path().to_path_buf();
    home.write(
        "state/acus/guard.jsonl",
        &jsonl(&[
            json!({"ts": 1, "level": "info", "event": "decision", "session": "s", "cwd": "c",
            "tool": "Bash", "rule": "grep", "kind": "rewrite", "call": "x"}),
        ]),
    );
    let out = census(home.path(), &cwd, &["usage", "--guard", "--json"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    let rules = v["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["rewrite"], 1);
}

#[test]
fn guard_log_reads_on_past_a_line_that_is_not_utf8() {
    let home = Scratch::new("cli-guard-utf8");
    let cwd = home.path().to_path_buf();
    let mut bytes =
        jsonl(&[json!({"ts": 20, "rule": "grep", "kind": "hint", "call": "grep b"})]).into_bytes();
    bytes.extend_from_slice(b"\xff\xfe not utf-8\n");
    bytes.extend_from_slice(
        jsonl(&[json!({"ts": 30, "rule": "sed", "kind": "escape", "call": "sed -i x"})]).as_bytes(),
    );
    home.write_bytes("state/acus/guard.jsonl", &bytes);
    let out = census(home.path(), &cwd, &["usage", "--guard", "--json"], "", &[]);
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    let rules = v["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["hint"], 1);
    assert_eq!(rules[1]["escape"], 1);
}

#[test]
fn guard_without_a_log_prints_the_hint_and_exits_1() {
    let home = Scratch::new("cli-guard-empty");
    let cwd = home.path().to_path_buf();
    let out = census(home.path(), &cwd, &["usage", "--guard"], "", &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("acus self setup"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn guard_json_without_a_log_is_the_empty_report() {
    let home = Scratch::new("cli-guard-empty-json");
    let cwd = home.path().to_path_buf();
    let out = census(home.path(), &cwd, &["usage", "--guard", "--json"], "", &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    assert_eq!(v, json!({"rules": [], "escaped": []}));
}

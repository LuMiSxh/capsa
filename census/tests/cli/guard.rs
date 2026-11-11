// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn guard_log_counts_rotated_and_current_lines_and_skips_bad_ones() {
    let home = Scratch::new("cli-guard");
    let cwd = home.path().to_path_buf();
    home.write(
        "acus/guard.jsonl.old",
        &jsonl(&[json!({"ts": 10, "rule": "grep", "kind": "deny", "call": "grep a"})]),
    );
    home.write(
        "acus/guard.jsonl",
        &format!(
            "{}not json\n{}",
            jsonl(&[json!({"ts": 20, "rule": "grep", "kind": "hint", "call": "grep b"})]),
            jsonl(&[json!({"ts": 30, "rule": "sed", "kind": "escape", "call": "sed -i x"})]),
        ),
    );
    let config = home.path().join("acus/config.toml").display().to_string();
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--json"],
        "",
        &[("ACUS_CONFIG", config.as_str())],
    );
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
        "acus/guard.jsonl",
        &jsonl(&[
            json!({"ts": 1, "level": "info", "event": "decision", "session": "s", "cwd": "c",
            "tool": "Bash", "rule": "grep", "kind": "rewrite", "call": "x"}),
        ]),
    );
    let config = home.path().join("acus/config.toml").display().to_string();
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--json"],
        "",
        &[("ACUS_CONFIG", config.as_str())],
    );
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
    home.write_bytes("acus/guard.jsonl", &bytes);
    let config = home.path().join("acus/config.toml").display().to_string();
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--json"],
        "",
        &[("ACUS_CONFIG", config.as_str())],
    );
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    let rules = v["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["hint"], 1);
    assert_eq!(rules[1]["escape"], 1);
}

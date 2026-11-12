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

fn five_hints_and_a_later_escape(home: &Scratch) {
    let mut lines: Vec<Value> = (0..5)
        .map(|i| {
            json!({"ts": 1000 + i, "session": format!("s{i}"), "rule": "grep",
            "kind": "hint", "call": "grep q"})
        })
        .collect();
    lines.push(
        json!({"ts": 100000, "session": "x", "rule": "sed", "kind": "escape",
        "call": "sed -i x"}),
    );
    home.write("state/acus/guard.jsonl", &jsonl(&lines));
}

#[test]
fn guard_follow_rate_shows_once_a_rule_has_five_observed_decisions() {
    let home = Scratch::new("cli-guard-follow");
    let cwd = home.path().to_path_buf();
    five_hints_and_a_later_escape(&home);
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--json"],
        "",
        &[("CENSUS_NOW", "1791964800")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    let rules = v["rules"].as_array().expect("rules");
    let grep = rules
        .iter()
        .find(|r| r["rule"] == "grep")
        .expect("grep rule");
    assert_eq!(grep["observed"], 5);
    assert_eq!(grep["follow"].as_f64(), Some(1.0));
    let sed = rules.iter().find(|r| r["rule"] == "sed").expect("sed rule");
    assert_eq!(sed["observed"], 0);
    assert!(sed["follow"].is_null());
}

#[test]
fn guard_writes_the_follow_rates_to_the_census_bulletin() {
    let home = Scratch::new("cli-guard-bulletin");
    let cwd = home.path().to_path_buf();
    five_hints_and_a_later_escape(&home);
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--json"],
        "",
        &[("CENSUS_NOW", "1791964800")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let path = home.path().join("state/capsa/bulletin/census.json");
    let bulletin = std::fs::read_to_string(&path).expect("the census bulletin is written");
    let b: Value = serde_json::from_str(&bulletin).expect("bulletin json");
    assert_eq!(b["guard_follow"]["at"], 1791964800);
    assert_eq!(b["guard_follow"]["ttl"], 604_800);
    assert_eq!(b["guard_follow"]["value"]["grep"].as_f64(), Some(1.0));
    assert!(b["guard_follow"]["value"].get("sed").is_none());
}

#[test]
fn guard_by_project_splits_each_rule_per_working_directory() {
    let home = Scratch::new("cli-guard-projects");
    let cwd = home.path().to_path_buf();
    home.write(
        "state/acus/guard.jsonl",
        &jsonl(&[
            json!({"ts": 1, "session": "s", "cwd": "/work/a", "rule": "grep", "kind": "deny", "call": "grep x"}),
            json!({"ts": 2, "session": "s", "cwd": "/work/b", "rule": "grep", "kind": "hint", "call": "grep y"}),
            json!({"ts": 3, "session": "s", "cwd": "/work/a", "rule": "sed", "kind": "escape", "call": "sed -i z"}),
            json!({"ts": 4, "session": "s", "cwd": "/work/a", "rule": "grep", "kind": "deny", "call": "grep w"}),
        ]),
    );
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--by-project", "--json"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("guard --json");
    let projects = v["projects"].as_array().expect("projects");
    assert_eq!(projects.len(), 3);
    assert_eq!(projects[0]["project"], "a");
    assert_eq!(projects[0]["rule"], "grep");
    assert_eq!(projects[0]["deny"], 2);
    assert_eq!(projects[1]["rule"], "sed");
    assert_eq!(projects[1]["escape"], 1);
    assert_eq!(projects[2]["project"], "b");
    assert_eq!(projects[2]["hint"], 1);
    assert_eq!(v["rules"].as_array().map(Vec::len), Some(2));
}

#[test]
fn guard_by_project_names_the_rows_it_leaves_out() {
    let home = Scratch::new("cli-guard-projects-top");
    let cwd = home.path().to_path_buf();
    home.write(
        "state/acus/guard.jsonl",
        &jsonl(&[
            json!({"ts": 1, "session": "s", "cwd": "/work/a", "rule": "grep", "kind": "deny", "call": "grep x"}),
            json!({"ts": 2, "session": "s", "cwd": "/work/b", "rule": "grep", "kind": "deny", "call": "grep y"}),
        ]),
    );
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--guard", "--by-project", "--top", "1"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("census usage --guard --by-project --top N"),
        "{}",
        text(&out.stdout)
    );
}

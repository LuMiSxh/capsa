// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::state::tool_state;

#[test]
fn scripts_report_counts_runs_failures_and_median_in_the_window() {
    let home = Scratch::new("cli-scripts");
    let cwd = home.path().to_path_buf();
    let now = 1_791_964_800u64;
    let run = |ts: u64, name: &str, scope: &str, cwd: &str, ms: u64, ok: bool| {
        json!({"ts": ts, "level": "info", "event": "run", "name": name, "version": 1,
            "scope": scope, "cwd": cwd, "ms": ms, "ok": ok})
    };
    home.write(
        &tool_state("acus", "script.jsonl.old"),
        &jsonl(&[run(
            now - 2 * 86_400,
            "deploy",
            "repo",
            "/work/app",
            40,
            true,
        )]),
    );
    home.write(
        &tool_state("acus", "script.jsonl"),
        &jsonl(&[
            run(now - 3_600, "deploy", "repo", "/work/app", 20, true),
            run(now - 1_800, "deploy", "repo", "/work/app", 60, false),
            run(
                now - 100 * 86_400,
                "deploy",
                "repo",
                "/work/app",
                9_999,
                false,
            ),
            run(now - 600, "audit", "user", "/work/other", 10, true),
        ]),
    );
    let out = census(
        home.path(),
        &cwd,
        &["usage", "--scripts", "--days", "30", "--json"],
        "",
        &[("CENSUS_NOW", "1791964800")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("scripts --json");
    assert_eq!(
        v["scripts"],
        json!([
            {"name": "deploy", "scope": "repo", "runs": 3, "failures": 1, "median_ms": 40,
                "last_ts": now - 1_800, "repo": "app"},
            {"name": "audit", "scope": "user", "runs": 1, "failures": 0, "median_ms": 10,
                "last_ts": now - 600, "repo": "other"},
        ])
    );
}

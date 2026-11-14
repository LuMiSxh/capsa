// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn prune_needs_exactly_one_cutoff() {
    let home = Scratch::new("cli-prune-args");
    let cwd = home.path().to_path_buf();
    let none = census(home.path(), &cwd, &["prune"], "", &[]);
    assert_eq!(none.status.code(), Some(2));
    let both = census(
        home.path(),
        &cwd,
        &["prune", "--before", "2026-01-01", "--keep", "5"],
        "",
        &[],
    );
    assert_eq!(both.status.code(), Some(2));
}

#[test]
fn prune_rejects_a_bad_date_and_zero_days() {
    let home = Scratch::new("cli-prune-values");
    let cwd = home.path().to_path_buf();
    let date = census(
        home.path(),
        &cwd,
        &["prune", "--before", "2026-13-45"],
        "",
        &[],
    );
    assert_eq!(date.status.code(), Some(2));
    assert!(text(&date.stderr).contains("YYYY-MM-DD"));
    let days = census(home.path(), &cwd, &["prune", "--keep", "0"], "", &[]);
    assert_eq!(days.status.code(), Some(2));
    assert!(text(&days.stderr).contains("at least 1 day"));
}

#[test]
fn prune_dry_run_on_an_empty_ledger_removes_nothing() {
    let home = Scratch::new("cli-prune-empty");
    let cwd = home.path().to_path_buf();
    let out = census(
        home.path(),
        &cwd,
        &["prune", "--keep", "365", "--dry-run"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stdout),
        "would remove 0 hourly aggregates and 0 snapshots older than 365 days\n"
    );
}

#[test]
fn budget_mode_prune_keeps_the_current_months_aggregates_and_spend() {
    let home = Scratch::new("cli-prune-month");
    october_spend(&home, "budget");
    let owned = isolated_env(home.path());
    let env: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &env);
    let spend = || -> f64 {
        serde_json::from_str::<Value>(&text(&run(&["--json", "limits"]).stdout))
            .expect("json")["spent_usd"]
            .as_f64()
            .expect("a month spend")
    };
    let before = spend();
    assert!(before > 0.0, "{before}");

    let by_date = run(&["--json", "prune", "--before", "2026-10-10"]);
    assert_eq!(by_date.status.code(), Some(1));
    assert_eq!(removed(&by_date), 0);
    assert!(text(&by_date.stderr).contains("budget mode keeps"));

    let by_days = run(&["--json", "prune", "--keep", "1"]);
    assert_eq!(by_days.status.code(), Some(1));
    assert_eq!(removed(&by_days), 0);

    assert_eq!(spend(), before);
}

#[test]
fn plan_mode_prune_removes_the_october_aggregates() {
    let home = Scratch::new("cli-prune-plan");
    october_spend(&home, "plan");
    let owned = isolated_env(home.path());
    let env: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let out = census(
        home.path(),
        home.path(),
        &["--json", "prune", "--before", "2026-10-10"],
        "",
        &env,
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(removed(&out), 2);
    assert!(!text(&out.stderr).contains("budget mode"));
}

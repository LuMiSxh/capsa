// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn daily_limit_is_set_shown_and_cleared() {
    let home = Scratch::new("cli-daily-set");
    let cfg = home.path().join("config");
    let state = home.path().join("state");
    let extra = [
        ("XDG_CONFIG_HOME", cfg.to_str().expect("utf-8 path")),
        ("APPDATA", cfg.to_str().expect("utf-8 path")),
        ("LOCALAPPDATA", state.to_str().expect("utf-8 path")),
    ];
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &extra);
    let saved = || -> String {
        std::fs::read_to_string(home.path().join(CONFIG_FILE)).expect("config file")
    };

    assert_eq!(
        run(&["self", "config", "set", "mode", "budget"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["self", "config", "set", "monthly-budget", "100"])
            .status
            .code(),
        Some(0)
    );
    let set = run(&["self", "config", "set", "daily-limit", "8"]);
    assert_eq!(set.status.code(), Some(0));
    assert!(text(&set.stdout).contains("$8.00"), "{}", text(&set.stdout));
    assert!(saved().contains("daily_limit_usd = 8.0"), "{}", saved());
    assert_eq!(
        run(&["self", "config", "set", "daily-limit", "0"])
            .status
            .code(),
        Some(2)
    );
    let cleared = run(&["self", "config", "set", "daily-limit", "auto"]);
    assert_eq!(cleared.status.code(), Some(0));
    assert!(!saved().contains("daily_limit_usd"), "{}", saved());
    assert!(
        text(&cleared.stdout).contains("auto"),
        "{}",
        text(&cleared.stdout)
    );
}

#[test]
fn budget_limits_json_carries_today_and_its_daily_limit() {
    let home = Scratch::new("cli-daily-json");
    october_spend(&home, "budget");
    let env = isolated_env(home.path());
    let extra: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let limits = |extra: &[(&str, &str)]| {
        let out = census(home.path(), home.path(), &["--json", "limits"], "", extra);
        assert_eq!(out.status.code(), Some(0));
        serde_json::from_slice::<Value>(&out.stdout).expect("json")
    };

    let derived = limits(&extra);
    assert_eq!(derived["today"]["source"], "derived");
    assert_eq!(derived["today"]["usd"], 0.0);
    assert!(derived["today"]["limit"].as_f64().is_some_and(|l| l > 0.0));

    home.write(
        CONFIG_FILE,
        "mode = \"budget\"\nmonthly_budget_usd = 100.0\ndaily_limit_usd = 8.0\n",
    );
    let fixed = limits(&extra);
    assert_eq!(fixed["today"]["source"], "fixed");
    assert_eq!(fixed["today"]["limit"], 8.0);
}

#[test]
fn limits_and_usage_by_without_data_exit_one() {
    let home = Scratch::new("limits-empty");
    let out = census(home.path(), home.path(), &["limits"], "", &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stdout).contains("no usage data yet"));
    let out = census(home.path(), home.path(), &["--json", "limits"], "", &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stdout).contains("\"data\":false"));
    let out = census(
        home.path(),
        home.path(),
        &["usage", "--by", "cycle"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn cycle_without_a_mapped_reset_prints_the_note_instead_of_zero_buckets() {
    let home = Scratch::new("cli-cycle-unmapped");
    let cwd = home.path().to_path_buf();
    claude_session(&home, &cwd);
    for args in [
        &["usage", "--by", "cycle"][..],
        &["--human", "usage", "--by", "cycle"][..],
    ] {
        let out = census(home.path(), &cwd, args, "", &[]);
        assert_eq!(out.status.code(), Some(0));
        let output = text(&out.stdout);
        assert!(output.contains("needs two weeks of weekly snapshots to map the reset"));
        assert!(
            !output.contains("+0h"),
            "no bucket rows before the reset is mapped: {output}"
        );
    }
    let out = census(
        home.path(),
        &cwd,
        &["--json", "usage", "--by", "cycle"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0));
    let doc: Value = serde_json::from_slice(&out.stdout).expect("census prints JSON");
    assert_eq!(doc["basis"]["pattern"], "weekday_hour");
    assert_eq!(doc["rows"].as_array().map(Vec::len), Some(168));
}

#[test]
fn budget_mode_switches_limits_and_statusline() {
    let home = Scratch::new("cli-budget");
    let cfg = home.path().join("config");
    let state = home.path().join("state");
    home.write("claude/settings.json", r#"{"autoCompactWindow": 400000}"#);
    let extra = [
        ("XDG_CONFIG_HOME", cfg.to_str().expect("utf-8 path")),
        ("APPDATA", cfg.to_str().expect("utf-8 path")),
        ("LOCALAPPDATA", state.to_str().expect("utf-8 path")),
    ];
    let run = |args: &[&str], stdin: &str| census(home.path(), home.path(), args, stdin, &extra);

    let shown = text(&run(&["self", "config", "show"], "").stdout);
    assert!(shown.contains("plan"), "{shown}");
    assert_eq!(
        run(&["self", "config", "set", "mode", "budget"], "")
            .status
            .code(),
        Some(0)
    );
    let set = run(&["self", "config", "set", "monthly-budget", "200"], "");
    assert_eq!(set.status.code(), Some(0));
    assert!(text(&set.stdout).contains("$200.00"));
    assert_eq!(
        run(&["self", "config", "set", "monthly-budget", "-5"], "")
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(&["self", "config", "set", "mode", "free"], "")
            .status
            .code(),
        Some(2)
    );

    let json: Value =
        serde_json::from_str(&text(&run(&["--json", "limits"], "").stdout)).expect("json");
    assert_eq!(json["mode"], "budget");
    assert_eq!(json["budget_usd"], 200.0);

    let line = run(&["statusline", "--line"], STATUS);
    assert_eq!(text(&line.stdout), "ctx 44k/400k · $0/$200 0% →$0\n");

    run(&["self", "config", "set", "mode", "plan"], "");
    let line = run(&["statusline", "--line"], STATUS);
    assert_eq!(text(&line.stdout), "ctx 44k/400k · 5h 6% · wk 1%\n");
}

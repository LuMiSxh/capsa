// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn set_billing_stores_the_provider_and_warns_about_an_unknown_one() {
    let home = Scratch::new("cli-billing-set");
    let cfg = home.path().join("config");
    let state = home.path().join("state");
    let extra = [
        ("XDG_CONFIG_HOME", cfg.to_str().expect("utf-8 path")),
        ("APPDATA", cfg.to_str().expect("utf-8 path")),
        ("LOCALAPPDATA", state.to_str().expect("utf-8 path")),
    ];
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &extra);

    let shown = run(&["self", "config", "show"]);
    assert!(
        text(&shown.stdout).contains("anthropic subscription"),
        "{}",
        text(&shown.stdout)
    );

    let set = run(&["self", "config", "set", "billing", "openai", "subscription"]);
    assert_eq!(set.status.code(), Some(0), "{}", text(&set.stderr));
    assert!(
        text(&set.stdout).contains("anthropic subscription, openai subscription"),
        "{}",
        text(&set.stdout)
    );
    assert!(
        !text(&set.stdout).contains("warning"),
        "{}",
        text(&set.stdout)
    );

    let unknown = run(&["self", "config", "set", "billing", "mistral", "api"]);
    assert_eq!(unknown.status.code(), Some(0), "{}", text(&unknown.stderr));
    assert!(
        text(&unknown.stdout).contains("`mistral` is not in the price table"),
        "{}",
        text(&unknown.stdout)
    );

    let saved = std::fs::read_to_string(home.path().join(CONFIG_FILE)).expect("config file");
    for line in [
        "anthropic = \"subscription\"",
        "openai = \"subscription\"",
        "mistral = \"api\"",
    ] {
        assert!(saved.contains(line), "{saved}");
    }

    let anthropic = run(&[
        "--json",
        "self",
        "config",
        "set",
        "billing",
        "anthropic",
        "api",
    ]);
    assert_eq!(
        anthropic.status.code(),
        Some(0),
        "{}",
        text(&anthropic.stderr)
    );
    let json: Value = serde_json::from_str(&text(&anthropic.stdout)).expect("json");
    assert_eq!(json["billing"]["anthropic"], "api");
    assert!(json.get("warnings").is_none(), "{json}");
}

#[test]
fn set_billing_refuses_a_bad_provider_or_billing_with_a_hint() {
    let home = Scratch::new("cli-billing-bad");
    let run = |args: &[&str]| census(home.path(), home.path(), args, "", &[]);
    for args in [
        ["self", "config", "set", "billing", "OpenAI", "api"],
        ["self", "config", "set", "billing", "openai", "free"],
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("hint:"), "{}", text(&out.stderr));
    }
    assert!(!home.path().join(CONFIG_FILE).exists());
}

#[test]
fn usage_names_a_provider_billed_as_subscription_by_the_setting() {
    let home = Scratch::new("cli-billing-usage");
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            json!({"type": "assistant", "timestamp": "2026-10-14T08:00:10.000Z",
                "message": {"id": "m1", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 1000, "output_tokens": 10}}}),
            json!({"type": "assistant", "timestamp": "2026-10-14T08:01:10.000Z",
                "message": {"id": "m2", "model": "gpt-5",
                    "usage": {"input_tokens": 1000, "output_tokens": 10}}}),
        ]),
    );
    let providers = |out: &Output| -> Vec<String> {
        text(&out.stdout)
            .lines()
            .find(|l| l.trim_start().starts_with("providers"))
            .map(|l| {
                l.trim_start()["providers".len()..]
                    .trim_start_matches(':')
                    .trim()
                    .split(", ")
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };

    let plain = census(home.path(), home.path(), &["usage"], "", &[]);
    assert_eq!(plain.status.code(), Some(0), "{}", text(&plain.stderr));
    let row = providers(&plain);
    assert!(
        row.iter()
            .any(|p| p.starts_with("openai $") && p.ends_with("(API)")),
        "{row:?}"
    );

    home.write(
        CONFIG_FILE,
        "[billing]\nanthropic = \"subscription\"\nopenai = \"subscription\"\n",
    );
    let set = census(home.path(), home.path(), &["usage"], "", &[]);
    assert_eq!(set.status.code(), Some(0), "{}", text(&set.stderr));
    let row = providers(&set);
    assert!(
        row.iter()
            .any(|p| p.starts_with("openai $") && p.ends_with("(subscription)")),
        "{row:?}"
    );
}

#[test]
fn limits_ignore_a_provider_billed_as_subscription() {
    let home = Scratch::new("cli-billing-limits");
    let request = |model: &str, id: &str, at: &str| {
        json!({"type": "assistant", "timestamp": at,
            "message": {"id": id, "model": model,
                "usage": {"input_tokens": 400_000, "output_tokens": 10}}})
    };
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            request("claude-opus-5-5", "m1", "2026-10-02T10:00:00.000Z"),
            request("gpt-5", "m2", "2026-10-03T10:00:00.000Z"),
        ]),
    );
    let owned = isolated_env(home.path());
    let env: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let spent = |billing: &str| -> f64 {
        home.write(
            CONFIG_FILE,
            &format!("mode = \"budget\"\nmonthly_budget_usd = 100.0\n{billing}"),
        );
        let out = census(home.path(), home.path(), &["--json", "limits"], "", &env);
        let json: Value = serde_json::from_str(&text(&out.stdout)).expect("json");
        json["spent_usd"].as_f64().expect("spent_usd")
    };
    let api = spent("[billing]\nanthropic = \"subscription\"\n");
    let subscription =
        spent("[billing]\nanthropic = \"subscription\"\nopenai = \"subscription\"\n");
    assert!(api > 0.0, "{api}");
    assert_eq!(subscription, api);
}

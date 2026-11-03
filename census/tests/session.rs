// SPDX-License-Identifier: MPL-2.0

mod common;

use census::config::{Billing, Config};
use census::session::{Picked, SessionError, resolve, summarize};
use common::Scratch;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

#[expect(
    clippy::expect_used,
    reason = "a fixture whose time cannot be set must fail the test"
)]
fn set_mtime(path: &Path, secs: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open the fixture")
        .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
        .expect("set the modification time");
}

/// A transcript whose first record names `cwd`.
fn transcript(cwd: &str) -> String {
    format!(
        "{{\"type\":\"user\",\"cwd\":\"{cwd}\",\"timestamp\":\"2026-10-14T08:00:00.000Z\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n"
    )
}

/// Three main transcripts and one subagent transcript, with their modification times.
fn projects(scratch: &Scratch) -> Vec<PathBuf> {
    let one = scratch.write("projects/A/one.jsonl", &transcript("/work/app"));
    let two = scratch.write("projects/B/two.jsonl", &transcript("/work/other"));
    let three = scratch.write("projects/C/three.jsonl", &transcript("/work/app"));
    let sub = scratch.write(
        "projects/A/one/subagents/agent-x.jsonl",
        &transcript("/work/app"),
    );
    set_mtime(&one, 1000);
    set_mtime(&two, 2000);
    set_mtime(&three, 1500);
    set_mtime(&sub, 5000);
    vec![scratch.path().join("projects")]
}

#[test]
fn the_newest_transcript_for_the_directory_is_picked() {
    let scratch = Scratch::new("recent");
    let roots = projects(&scratch);
    let found = resolve(&roots, Path::new("/work/app/src"), None, None).expect("a session");
    assert_eq!(found.id, "three");
    assert_eq!(found.picked, Picked::Recent);
}

#[test]
fn a_directory_above_or_below_the_recorded_one_matches() {
    let scratch = Scratch::new("related");
    let roots = projects(&scratch);
    // From /work, the newest transcript is in /work/other, a descendant, so it wins over /work/app.
    let above = resolve(&roots, Path::new("/work"), None, None).expect("a session");
    assert_eq!(above.id, "two");
    assert!(matches!(
        resolve(&roots, Path::new("/elsewhere"), None, None),
        Err(SessionError::NoSession(_))
    ));
}

#[test]
fn subagent_transcripts_are_never_chosen() {
    let scratch = Scratch::new("subagent");
    let roots = projects(&scratch);
    let found = resolve(&roots, Path::new("/work/app"), None, None).expect("a session");
    assert_ne!(found.id, "agent-x");
}

#[test]
fn a_flag_or_the_environment_names_a_session_by_id() {
    let scratch = Scratch::new("ids");
    let roots = projects(&scratch);
    let cwd = Path::new("/nowhere");
    let flag = resolve(&roots, cwd, Some("one"), Some("two")).expect("flag wins");
    assert_eq!((flag.id.as_str(), flag.picked), ("one", Picked::Flag));
    let env = resolve(&roots, cwd, None, Some("two")).expect("env names a session");
    assert_eq!((env.id.as_str(), env.picked), ("two", Picked::Env));
    assert!(matches!(
        resolve(&roots, cwd, Some("missing"), None),
        Err(SessionError::UnknownId(id)) if id == "missing"
    ));
    assert!(matches!(
        resolve(&roots, cwd, None, Some("")),
        Err(SessionError::NoSession(_))
    ));
}

#[test]
fn summary_counts_requests_roles_compactions_and_context() {
    let scratch = Scratch::new("summary");
    let main = scratch.write(
        "projects/app/s1.jsonl",
        concat!(
            r#"{"type":"user","cwd":"/work/app","timestamp":"2026-10-14T08:00:00.000Z","message":{"role":"user","content":"go"}}"#, "\n",
            r#"{"type":"assistant","timestamp":"2026-10-14T08:00:10.000Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":1000,"cache_creation_input_tokens":100,"output_tokens":50}}}"#, "\n",
            r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-10-14T08:01:00.000Z"}"#, "\n",
            r#"{"type":"assistant","timestamp":"2026-10-14T08:02:00.000Z","message":{"id":"m2","model":"claude-opus-5-5","usage":{"input_tokens":5,"cache_read_input_tokens":2000,"output_tokens":20}}}"#, "\n",
            r#"{"type":"assistant","timestamp":"2026-10-14T08:02:01.000Z","message":{"id":"m2","model":"claude-opus-5-5","usage":{"input_tokens":5,"cache_read_input_tokens":2000,"output_tokens":20}}}"#, "\n",
        ),
    );
    scratch.write(
        "projects/app/s1/subagents/agent-a.jsonl",
        r#"{"type":"assistant","attributionAgent":"worker","timestamp":"2026-10-14T08:03:00.000Z","message":{"id":"m3","model":"claude-sonnet-5-5","usage":{"input_tokens":100,"output_tokens":10}}}"#,
    );
    scratch.write(
        "projects/app/s1/subagents/agent-b.jsonl",
        concat!(
            r#"{"type":"assistant","attributionAgent":"explore","timestamp":"2026-10-14T08:02:30.000Z","message":{"id":"m4","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#, "\n",
            r#"{"type":"assistant","attributionAgent":"explore","timestamp":"2026-10-14T08:02:40.000Z","message":{"id":"m5","model":"claude-sonnet-5-5","usage":{"input_tokens":1,"output_tokens":1}}}"#, "\n",
        ),
    );

    let s = summarize(&main, &Config::default()).expect("the summary");
    assert_eq!(s.cwd.as_deref(), Some("/work/app"));
    assert_eq!(s.requests, 4);
    assert_eq!(s.subagents, 2);
    assert_eq!(
        s.spawns,
        BTreeMap::from([("explore".to_owned(), 1), ("worker".to_owned(), 1)])
    );
    assert_eq!(s.compactions, 1);
    assert_eq!(s.duration_s, 180);
    assert_eq!(s.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(s.cost.unpriced, 0);
    assert!(s.cost.usd > 0.0);
    assert_eq!(s.context.map(|c| c.tokens), Some(2005));
}

#[test]
fn a_deepseek_subagent_is_api_spend_apart_from_the_subscription_cost() {
    let scratch = Scratch::new("api");
    let claude = r#"{"type":"assistant","timestamp":"2026-10-14T08:00:10.000Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":50}}}"#;
    let alone = scratch.write("projects/app/s1.jsonl", &format!("{claude}\n"));
    let with_api = scratch.write("projects/app/s2.jsonl", &format!("{claude}\n"));
    scratch.write(
        "projects/app/s2/subagents/agent-c.jsonl",
        r#"{"type":"assistant","attributionAgent":"worker","timestamp":"2026-10-14T08:03:00.000Z","message":{"id":"m3","model":"deepseek-v4-pro","usage":{"input_tokens":1000,"output_tokens":100}}}"#,
    );

    let subscription = summarize(&alone, &Config::default()).expect("the summary");
    let mixed = summarize(&with_api, &Config::default()).expect("the summary");
    assert_eq!(mixed.requests, 2);
    assert!((mixed.cost.usd - subscription.cost.usd).abs() < 1e-12);
    assert_eq!(mixed.cost.unpriced, 0);
    assert_eq!(mixed.api.keys().copied().collect::<Vec<_>>(), ["deepseek"]);
    assert!(mixed.api["deepseek"].usd > 0.0);
    assert!(subscription.api.is_empty());
}

#[test]
fn a_provider_billed_as_subscription_moves_its_spend_from_the_api_rows_to_the_cost() {
    let scratch = Scratch::new("billing");
    let claude = r#"{"type":"assistant","timestamp":"2026-10-14T08:00:10.000Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":50}}}"#;
    let main = scratch.write("projects/app/s1.jsonl", &format!("{claude}\n"));
    scratch.write(
        "projects/app/s1/subagents/agent-d.jsonl",
        r#"{"type":"assistant","attributionAgent":"worker","timestamp":"2026-10-14T08:03:00.000Z","message":{"id":"m4","model":"gpt-5","usage":{"input_tokens":1000,"output_tokens":100}}}"#,
    );

    let as_api = summarize(&main, &Config::default()).expect("the summary");
    let openai = Config {
        billing: BTreeMap::from([
            ("anthropic".to_owned(), Billing::Subscription),
            ("openai".to_owned(), Billing::Subscription),
        ]),
        ..Config::default()
    };
    let as_subscription = summarize(&main, &openai).expect("the summary");
    assert_eq!(as_api.api.keys().copied().collect::<Vec<_>>(), ["openai"]);
    assert!(as_subscription.api.is_empty());
    let api_usd = as_api.api["openai"].usd;
    assert!(api_usd > 0.0);
    assert!((as_subscription.cost.usd - (as_api.cost.usd + api_usd)).abs() < 1e-9);
}

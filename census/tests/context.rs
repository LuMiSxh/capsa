// SPDX-License-Identifier: MPL-2.0

mod common;

use census::context::{Threshold, at, last_context, model_window, settings_window, threshold};
use common::Scratch;
use std::collections::HashMap;

/// A variable lookup over fixed pairs, standing in for the process environment.
fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let vars: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |key| vars.get(key).cloned()
}

#[test]
fn the_threshold_follows_the_env_settings_and_the_model_window() {
    type Case = (
        Option<&'static str>,
        &'static [(&'static str, &'static str)],
        Option<u64>,
        fn(&Threshold),
    );
    let cases: &[Case] = &[
        (
            Some("claude-opus-5-5"),
            &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "300000")],
            Some(400_000),
            |t| {
                assert_eq!(t.tokens, 300_000);
                assert!(t.source.contains("CLAUDE_CODE_AUTO_COMPACT_WINDOW"));
            },
        ),
        (Some("claude-opus-5-5"), &[], Some(400_000), |t| {
            assert_eq!(t.tokens, 400_000);
            assert_eq!(t.source, "autoCompactWindow in settings.json");
        }),
        (Some("claude-opus-5-5"), &[], None, |t| {
            assert_eq!(t.tokens, 1_000_000);
            assert_eq!(t.source, "default (approximate)");
        }),
        (
            Some("claude-haiku-4-5"),
            &[("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "2000000")],
            None,
            |t| assert_eq!(t.tokens, 200_000),
        ),
        (Some("claude-opus-5-5"), &[], Some(1_500_000), |t| {
            assert_eq!(t.tokens, 1_000_000);
        }),
        (
            Some("claude-opus-5-5"),
            &[("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", "50")],
            Some(400_000),
            |t| {
                assert_eq!(t.tokens, 200_000);
                assert!(t.source.contains("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE 50%"));
            },
        ),
    ];
    for (model, vars, settings, check) in cases {
        check(&threshold(*model, env(vars), *settings));
    }

    for ignored in ["0", "101", "abc"] {
        let t = threshold(
            Some("claude-opus-5-5"),
            env(&[("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", ignored)]),
            Some(400_000),
        );
        assert_eq!(t.tokens, 400_000, "override {ignored:?} must be ignored");
    }
}

#[test]
fn model_windows_follow_the_families_and_versions() {
    let large = [
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "claude-opus-4-6",
        "claude-sonnet-4-6",
        "claude-opus-4-7",
        "claude-fable-1",
    ];
    for model in large {
        assert_eq!(model_window(Some(model)), 1_000_000, "{model}");
    }
    let small = [
        "claude-sonnet-4-5-20250929",
        "claude-3-5-sonnet-20241022",
        "claude-haiku-4-5",
        "claude-opus-4-1-20250805",
        "gpt-x",
    ];
    for model in small {
        assert_eq!(model_window(Some(model)), 200_000, "{model}");
    }
    assert_eq!(model_window(None), 200_000);
}

#[test]
fn settings_window_reads_auto_compact_window_from_the_config_dir() {
    let scratch = Scratch::new("settings");
    assert_eq!(settings_window(scratch.path()), None);
    scratch.write("settings.json", r#"{"autoCompactWindow": 400000}"#);
    assert_eq!(settings_window(scratch.path()), Some(400_000));
}

#[test]
fn last_context_skips_synthetic_messages() {
    let scratch = Scratch::new("transcript");
    let text = concat!(
        r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":3,"cache_read_input_tokens":1000,"cache_creation_input_tokens":200,"output_tokens":5}}}"#,
        "\n",
        r#"{"type":"assistant","message":{"id":"m2","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#,
        "\n",
    );
    let path = scratch.write("s.jsonl", text);
    assert_eq!(
        last_context(&path),
        Some(("claude-opus-5-5".to_owned(), 1203))
    );
    assert_eq!(last_context(&scratch.path().join("missing.jsonl")), None);
}

/// A request of `claude-opus-5-5` with 1,203 context tokens, as one line.
fn request_line(id: &str) -> String {
    let usage = r#""usage":{"input_tokens":3,"cache_read_input_tokens":1000,"cache_creation_input_tokens":200,"output_tokens":5}"#;
    format!(r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude-opus-5-5",{usage}}}}}"#)
        + "\n"
}

/// About `kib` KiB of user lines that hold no API request.
fn filler(kib: usize) -> String {
    let content = "x".repeat(1000);
    let line = format!(r#"{{"type":"user","message":{{"role":"user","content":"{content}"}}}}"#);
    format!("{line}\n").repeat(kib)
}

#[test]
fn a_request_beyond_the_first_tail_is_found_in_the_fallback_windows() {
    let scratch = Scratch::new("transcript-fallback");
    let near = scratch.write("near.jsonl", &(request_line("m1") + &filler(1024)));
    assert_eq!(
        last_context(&near),
        Some(("claude-opus-5-5".to_owned(), 1203))
    );
    let deep = scratch.write("deep.jsonl", &(request_line("m1") + &filler(5 * 1024)));
    assert_eq!(
        last_context(&deep),
        Some(("claude-opus-5-5".to_owned(), 1203))
    );
    // Beyond the 16 MiB fallback the context is unknown, rather than read from the whole file.
    let lost = scratch.write("lost.jsonl", &(request_line("m1") + &filler(17 * 1024)));
    assert_eq!(last_context(&lost), None);
}

#[test]
fn at_reports_the_share_used_and_the_tokens_left() {
    let threshold = Threshold {
        tokens: 400_000,
        source: "test".to_owned(),
    };
    let used = at(100_000, Some("claude-opus-5-5"), threshold.clone());
    assert!((used.percent - 25.0).abs() < 1e-9);
    assert_eq!(used.left, 300_000);
    let passed = at(500_000, None, threshold);
    assert_eq!(passed.left, 0);
    assert!((passed.percent - 125.0).abs() < 1e-9);
}

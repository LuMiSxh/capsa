// SPDX-License-Identifier: MPL-2.0
use std::ffi::OsString;

use super::{Env, Format, Stream, agent_env_in, ago, color, color_in, color_stdout, resolve_in};

#[derive(Default)]
struct Fake(Vec<(&'static str, &'static str)>);

impl Fake {
    fn with(vars: &[(&'static str, &'static str)]) -> Self {
        Self(vars.to_vec())
    }
}

impl Env for Fake {
    fn var_os(&self, name: &str) -> Option<OsString> {
        self.0
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| OsString::from(*value))
    }

    fn any_with_prefix(&self, prefix: &str) -> bool {
        self.0.iter().any(|(key, _)| key.starts_with(prefix))
    }
}

fn pick(flags: (bool, bool, bool), vars: &[(&'static str, &'static str)]) -> Format {
    let (human, json, agent) = flags;
    resolve_in(human, json, agent, &Fake::with(vars))
}

#[test]
fn explicit_flags_win_in_json_human_agent_order() {
    assert_eq!(pick((true, true, true), &[]), Format::Json);
    assert_eq!(pick((true, false, true), &[]), Format::Human);
    assert_eq!(pick((false, false, true), &[]), Format::Agent);
}

#[test]
fn flags_beat_capsa_human() {
    let human_env = [("CAPSA_HUMAN", "1")];
    assert_eq!(pick((false, false, true), &human_env), Format::Agent);
    assert_eq!(pick((false, true, false), &human_env), Format::Json);
    assert_eq!(pick((true, false, false), &human_env), Format::Human);
}

#[test]
fn default_is_agent() {
    assert_eq!(pick((false, false, false), &[]), Format::Agent);
}

#[test]
fn capsa_human_selects_human_outside_agent_env() {
    assert_eq!(
        pick((false, false, false), &[("CAPSA_HUMAN", "1")]),
        Format::Human
    );
    assert_eq!(
        pick((false, false, false), &[("CAPSA_HUMAN", "0")]),
        Format::Agent
    );
}

#[test]
fn capsa_human_is_ignored_under_claude_code() {
    let vars = [("CAPSA_HUMAN", "1"), ("CLAUDECODE", "1")];
    assert_eq!(pick((false, false, false), &vars), Format::Agent);
}

#[test]
fn capsa_human_is_ignored_under_codex() {
    let vars = [("CAPSA_HUMAN", "1"), ("CODEX_HOME", "/x")];
    assert_eq!(pick((false, false, false), &vars), Format::Agent);
}

#[test]
fn agent_env_detects_claude_code_and_codex_prefix() {
    assert!(agent_env_in(&Fake::with(&[("CLAUDECODE", "1")])));
    assert!(agent_env_in(&Fake::with(&[("CODEX_SANDBOX", "seatbelt")])));
    assert!(!agent_env_in(&Fake::with(&[
        ("CODEXX", "1"),
        ("HOME", "/x")
    ])));
}

#[test]
fn agent_output_is_never_coloured_on_any_stream() {
    assert!(!color(Format::Agent, Stream::Stdout));
    assert!(!color(Format::Agent, Stream::Stderr));
    assert!(!color_stdout(Format::Json));
}

#[test]
fn colour_needs_human_a_terminal_and_no_no_color() {
    let plain = Fake::default();
    assert!(color_in(Format::Human, true, &plain));
    assert!(!color_in(Format::Human, false, &plain));
    assert!(!color_in(Format::Agent, true, &plain));
    assert!(!color_in(Format::Json, true, &plain));
    assert!(!color_in(
        Format::Human,
        true,
        &Fake::with(&[("NO_COLOR", "1")])
    ));
    assert!(color_in(
        Format::Human,
        true,
        &Fake::with(&[("NO_COLOR", "")])
    ));
}

#[test]
fn ago_reads_just_now_under_a_minute() {
    assert_eq!(ago(0), "just now");
    assert_eq!(ago(59), "just now");
}

#[test]
fn ago_uses_minutes_from_one_minute() {
    assert_eq!(ago(60), "1m ago");
    assert_eq!(ago(3599), "59m ago");
}

#[test]
fn ago_uses_hours_then_days() {
    assert_eq!(ago(3600), "1h ago");
    assert_eq!(ago(86_399), "23h ago");
    assert_eq!(ago(86_400), "1d ago");
    assert_eq!(ago(2_591_999), "29d ago");
}

#[test]
fn ago_uses_thirty_day_months() {
    assert_eq!(ago(2_592_000), "1mo ago");
    assert_eq!(ago(31_535_999), "12mo ago");
}

#[test]
fn ago_uses_365_day_years() {
    assert_eq!(ago(31_536_000), "1y ago");
    assert_eq!(ago(63_072_000), "2y ago");
}

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
fn explicit_flags_and_environment_pick_the_format() {
    const HUMAN: &[(&str, &str)] = &[("CAPSA_HUMAN", "1")];
    const NO_HUMAN: &[(&str, &str)] = &[("CAPSA_HUMAN", "0")];
    const CLAUDE: &[(&str, &str)] = &[("CAPSA_HUMAN", "1"), ("CLAUDECODE", "1")];
    const CODEX: &[(&str, &str)] = &[("CAPSA_HUMAN", "1"), ("CODEX_HOME", "/x")];
    type Case = (
        (bool, bool, bool),
        &'static [(&'static str, &'static str)],
        Format,
    );
    let cases: &[Case] = &[
        ((true, true, true), &[], Format::Json),
        ((true, false, true), &[], Format::Human),
        ((false, false, true), &[], Format::Agent),
        ((false, false, true), HUMAN, Format::Agent),
        ((false, true, false), HUMAN, Format::Json),
        ((true, false, false), HUMAN, Format::Human),
        ((false, false, false), &[], Format::Agent),
        ((false, false, false), HUMAN, Format::Human),
        ((false, false, false), NO_HUMAN, Format::Agent),
        ((false, false, false), CLAUDE, Format::Agent),
        ((false, false, false), CODEX, Format::Agent),
    ];
    for (flags, vars, expected) in cases {
        assert_eq!(pick(*flags, vars), *expected, "{flags:?} {vars:?}");
    }
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
fn colour_needs_human_a_terminal_and_no_no_color() {
    assert!(!color(Format::Agent, Stream::Stdout));
    assert!(!color(Format::Agent, Stream::Stderr));
    assert!(!color_stdout(Format::Json));

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
fn ago_uses_the_largest_unit_that_fits() {
    let cases = [
        (0, "just now"),
        (59, "just now"),
        (60, "1m ago"),
        (3599, "59m ago"),
        (3600, "1h ago"),
        (86_399, "23h ago"),
        (86_400, "1d ago"),
        (2_591_999, "29d ago"),
        (2_592_000, "1mo ago"),
        (31_535_999, "12mo ago"),
        (31_536_000, "1y ago"),
        (63_072_000, "2y ago"),
    ];
    for (seconds, expected) in cases {
        assert_eq!(ago(seconds), expected, "{seconds}");
    }
}

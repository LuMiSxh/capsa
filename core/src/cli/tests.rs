// SPDX-License-Identifier: MPL-2.0
use std::ffi::OsString;

use clap::Parser;

use super::{App, FormatArgs, raw_format_in};
use crate::format::{Env, Format};

struct Vars(Vec<(&'static str, &'static str)>);

impl Vars {
    fn with(pairs: &[(&'static str, &'static str)]) -> Self {
        Self(pairs.to_vec())
    }
}

impl Env for Vars {
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

#[derive(Debug, Parser)]
#[command(name = "probe", version = "1.2.3")]
struct Probe {
    #[command(flatten)]
    format: FormatArgs,
}

impl App for Probe {
    fn format(&self) -> Format {
        self.format.resolve()
    }
}

/// Parses `flags` after the program name and resolves them with `pairs` as the environment.
fn pick(flags: &[&str], pairs: &[(&'static str, &'static str)]) -> Format {
    let mut argv = vec!["probe"];
    argv.extend_from_slice(flags);
    Probe::try_parse_from(argv)
        .unwrap()
        .format
        .resolve_in(&Vars::with(pairs))
}

fn raw(argv: &[&str], pairs: &[(&'static str, &'static str)]) -> Format {
    let args: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
    raw_format_in(&args, &Vars::with(pairs))
}

#[test]
fn explicit_flags_win_over_the_environment() {
    let claude = [("CLAUDECODE", "1"), ("CAPSA_HUMAN", "1")];
    assert_eq!(pick(&["--json"], &claude), Format::Json);
    assert_eq!(pick(&["--human"], &claude), Format::Human);
    assert_eq!(pick(&["--agent"], &[("CAPSA_HUMAN", "1")]), Format::Agent);
    assert_eq!(pick(&["--format", "human"], &claude), Format::Human);
    assert_eq!(pick(&["--format=json"], &[]), Format::Json);
    assert_eq!(pick(&["--json", "--human"], &[]), Format::Json);
}

#[test]
fn capsa_human_selects_human_by_default() {
    assert_eq!(pick(&[], &[("CAPSA_HUMAN", "1")]), Format::Human);
}

#[test]
fn capsa_human_is_ignored_under_an_agent() {
    let claude = [("CAPSA_HUMAN", "1"), ("CLAUDECODE", "1")];
    let codex = [("CAPSA_HUMAN", "1"), ("CODEX_HOME", "/x")];
    assert_eq!(pick(&[], &claude), Format::Agent);
    assert_eq!(pick(&[], &codex), Format::Agent);
}

#[test]
fn capsa_human_needs_the_value_one() {
    assert_eq!(pick(&[], &[("CAPSA_HUMAN", "0")]), Format::Agent);
}

#[test]
fn agent_is_the_default_format() {
    assert_eq!(pick(&[], &[]), Format::Agent);
}

#[test]
fn raw_format_reads_the_flags_of_a_rejected_command_line() {
    assert_eq!(
        raw(&["probe", "--bogus", "--format=json"], &[]),
        Format::Json
    );
    assert_eq!(
        raw(&["probe", "--format", "human", "--bogus"], &[]),
        Format::Human
    );
    assert_eq!(
        raw(&["probe", "--agent", "--bogus"], &[("CAPSA_HUMAN", "1")]),
        Format::Agent
    );
    assert_eq!(
        raw(&["probe", "--bogus"], &[("CAPSA_HUMAN", "1")]),
        Format::Human
    );
    let claude = [("CAPSA_HUMAN", "1"), ("CLAUDECODE", "1")];
    assert_eq!(raw(&["probe", "--bogus"], &claude), Format::Agent);
}

#[test]
fn help_and_version_are_output_not_errors() {
    let help = Probe::try_parse_from(["probe", "--help"]).unwrap_err();
    assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
    assert!(!help.use_stderr());
    let version = Probe::try_parse_from(["probe", "--version"]).unwrap_err();
    assert_eq!(version.kind(), clap::error::ErrorKind::DisplayVersion);
    assert!(!version.use_stderr());
}

#[test]
fn parse_error_names_the_rejected_argument() {
    let err = Probe::try_parse_from(["probe", "--bogus"]).unwrap_err();
    let message = Probe::parse_error(&err, &[]).to_string();
    assert!(
        message.starts_with("unexpected argument '--bogus' found"),
        "{message}"
    );
}

#[test]
fn a_parsed_command_line_selects_its_format() {
    let probe = Probe::try_parse_from(["probe", "--json"]).unwrap();
    assert_eq!(probe.format(), Format::Json);
}

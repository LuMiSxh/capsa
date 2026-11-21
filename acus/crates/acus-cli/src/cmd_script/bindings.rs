// SPDX-License-Identifier: MPL-2.0
//! The functions a script can call: each turns its arguments into the command line of the acus
//! command of the same name, parses it with that command's own clap definition and returns the
//! command's computed report as a structured value.

use super::argv::Call;
use super::mcp::{self, Mcp};
use super::shapes;
use super::store::Store;
use crate::config::Config;
use crate::out::Out;
use crate::{Cli, Cmd, cmd_blame, cmd_diff, cmd_files, cmd_find, cmd_follow, cmd_log};
use crate::{cmd_outline, cmd_patch, cmd_run, cmd_show};
use acus_guard::{BashDecision, Scope, Verdict};
use acus_script::{Host, HostError};
use anyhow::Context;
use capsa_core::format::Format;
use capsa_core::hint::find_hint;
use clap::Parser;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const NAMES: &[&str] = &[
    "find",
    "show",
    "outline",
    "files",
    "follow",
    "diff",
    "log",
    "blame",
    "patch",
    "run",
    "read",
    mcp::CALL,
];

/// Seconds a `run()` command may take when the script gives no `timeout`: the host call is not
/// bounded by the script's deadline.
const RUN_TIMEOUT: u64 = 120;

pub(super) struct Bindings {
    cfg: Config,
    cwd: PathBuf,
    store: Store,
    mcp: Arc<Mcp>,
    /// Set when the script is only being checked for saving: no server may start.
    checking: bool,
}

impl Bindings {
    pub(super) fn new(cfg: Config) -> anyhow::Result<Self> {
        let cwd = std::env::current_dir().context("cannot read the current directory")?;
        let store = Store::open(&cfg, &cwd);
        let deadline = Instant::now() + super::limits(&cfg).deadline;
        let mcp = Arc::new(Mcp::new(
            cfg.script.mcp.clone(),
            cfg.path.clone(),
            cwd.clone(),
            deadline,
        ));
        Ok(Bindings {
            cfg,
            cwd,
            store,
            mcp,
            checking: false,
        })
    }

    /// Returns bindings for checking a script before it is saved: its `mcp:` imports load empty
    /// modules, so no server starts.
    pub(super) fn for_check(cfg: Config) -> anyhow::Result<Self> {
        Ok(Bindings {
            checking: true,
            ..Self::new(cfg)?
        })
    }

    /// Returns the MCP servers, so the caller can end them after the run.
    pub(super) fn mcp(&self) -> Arc<Mcp> {
        Arc::clone(&self.mcp)
    }

    fn find(&self, mut call: Call) -> Result<Value, HostError> {
        let patterns = std::mem::take(&mut call.positionals);
        call.options.insert("regexp".to_owned(), json!(patterns));
        let files_only = call.flag("filesWithMatches") || call.flag("l");
        let count = call.flag("count") || call.flag("c");
        let block = call.flag("block") || call.flag("b");
        let Cmd::Find(a) = parse("find", &call, &[])? else {
            return Err(internal("find"));
        };
        let report = cmd_find::compute(&a).map_err(|e| host_error(&e))?;
        Ok(if files_only || count {
            shapes::hit_files(&report, count)
        } else if block {
            shapes::blocks(&report)
        } else {
            shapes::hits(&report)
        })
    }

    fn patch(&self, mut call: Call, deadline: Instant) -> Result<Value, HostError> {
        let text = std::mem::take(&mut call.positionals).join("\n");
        let then = call.text("run").map(str::to_owned);
        let Cmd::Patch(a) = parse("patch", &call, &["run"])? else {
            return Err(internal("patch"));
        };
        let then = then.filter(|_| !call.flag("check"));
        if let Some(command) = &then {
            self.guard(command)?;
        }
        let report = cmd_patch::compute_text(&a, &text, &self.cfg).map_err(|e| host_error(&e))?;
        let mut value = shapes::patch(&report)?;
        if let Some(command) = then {
            value["run"] = self.run(Call::new("run", vec![json!(command)])?, deadline)?;
        }
        Ok(value)
    }

    fn run(&self, mut call: Call, deadline: Instant) -> Result<Value, HostError> {
        let [command] = call.positionals.as_slice() else {
            return Err(HostError::new("run() takes one command string")
                .with_hint("pass the command, then options: run('cargo test', {tail: 40})"));
        };
        self.guard(command)?;
        let json = call.flag("json");
        if json {
            call.options.insert("raw".to_owned(), json!(true));
            call.options
                .insert("maxOutput".to_owned(), json!(1_000_000));
        }
        clamp_timeout(
            &mut call.options,
            deadline.saturating_duration_since(Instant::now()),
        );
        let Cmd::Run(a) = parse("run", &call, &["json"])? else {
            return Err(internal("run"));
        };
        let report =
            cmd_run::compute(&a, &Out::new(Format::Json), &self.cfg).map_err(|e| host_error(&e))?;
        let mut value = shapes::run(&report)?;
        if json {
            let parsed = serde_json::from_str(value["text"].as_str().unwrap_or_default());
            value["json"] = parsed.map_err(|e| {
                HostError::new(format!("the command's output is not JSON: {e}")).with_hint(
                    "drop {json: true}, or print JSON only; stderr is merged into the output",
                )
            })?;
        }
        Ok(value)
    }

    /// Applies the decision the PreToolUse hook would give a Bash call; a refusal throws with the
    /// guard's reason as the hint. Nothing is rewritten, so a command the guard would fold into an
    /// acus call is refused and names it.
    fn guard(&self, command: &str) -> Result<(), HostError> {
        // `Scope` caches lazily without locking, so each call makes its own.
        let scope = Scope::new(self.cwd.to_str());
        match acus_guard::decide_bash(command, scope.as_ref(), false) {
            BashDecision::Verdict(Verdict::Deny(_, reason)) => Err(HostError::new(
                "run() refused the command: the acus guard blocks it",
            )
            .with_hint(reason)),
            _ => Ok(()),
        }
    }

    /// Reads a text file inside the repository, for data (JSON, TOML, lists) rather than code.
    fn read(&self, call: &Call) -> Result<Value, HostError> {
        let [path] = call.positionals.as_slice() else {
            return Err(HostError::new("read() takes one path"));
        };
        let full = self.cwd.join(path);
        let real = full
            .canonicalize()
            .map_err(|e| HostError::new(format!("cannot read {path}: {e}")))?;
        let Some(root) = self.cwd.ancestors().find(|d| d.join(".git").exists()) else {
            return Err(HostError::new("read() needs a git repository")
                .with_hint("run the script from inside a git repository"));
        };
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if !real.starts_with(root) {
            return Err(HostError::new(format!("{path} is outside the repository"))
                .with_hint("read() reads files under the repository root only"));
        }
        std::fs::read_to_string(&real)
            .map(Value::String)
            .map_err(|e| HostError::new(format!("cannot read {path}: {e}")))
    }
}

impl Host for Bindings {
    fn names(&self) -> &[&str] {
        NAMES
    }

    /// `patch` is the only call that writes: it waits for the calls before it and holds back the
    /// calls after it, so the order of the script's awaits is the order of the effects.
    fn exclusive(&self, name: &str) -> bool {
        name == "patch"
    }

    fn module(&self, specifier: &str) -> Result<String, HostError> {
        if let Some(server) = specifier.strip_prefix("mcp:") {
            return if self.checking {
                self.mcp.stub(server)
            } else {
                self.mcp.module(server)
            };
        }
        self.store.import(specifier).map_err(|e| host_error(&e))
    }

    fn call(&self, name: &str, args: Vec<Value>, deadline: Instant) -> Result<Value, HostError> {
        if self.cfg.commands.disabled.iter().any(|d| d == name) {
            return Err(HostError::new(format!(
                "{name} is disabled by configuration"
            )));
        }
        if name == mcp::CALL {
            return self.mcp.call(&args);
        }
        let call = Call::new(name, args)?;
        match name {
            "find" => self.find(call),
            "patch" => self.patch(call, deadline),
            "run" => self.run(call, deadline),
            "read" => self.read(&call),
            "show" => {
                let Cmd::Show(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                shapes::views(&cmd_show::compute(&a, Format::Json).map_err(|e| host_error(&e))?)
            }
            "outline" => {
                let Cmd::Outline(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                shapes::outline(&cmd_outline::compute(&a).map_err(|e| host_error(&e))?)
            }
            "files" => {
                let Cmd::Files(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                shapes::files(&cmd_files::compute(&a).map_err(|e| host_error(&e))?)
            }
            "follow" => {
                let Cmd::Follow(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                shapes::follow(&cmd_follow::compute(&a).map_err(|e| host_error(&e))?)
            }
            "diff" => {
                let Cmd::Diff(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                shapes::diff(&cmd_diff::compute(&a, true).map_err(|e| host_error(&e))?)
            }
            "log" => {
                let Cmd::Log(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                shapes::log(&cmd_log::compute(&a).map_err(|e| host_error(&e))?)
            }
            "blame" => {
                let Cmd::Blame(a) = parse(name, &call, &[])? else {
                    return Err(internal(name));
                };
                shapes::blame(&cmd_blame::compute(&a).map_err(|e| host_error(&e))?)
            }
            _ => Err(internal(name)),
        }
    }
}

/// Sets the command's `timeout` to the script's own or the default, cut to the whole seconds left
/// before the script's deadline (at least 1), so `run` kills the command's process group itself
/// before the script dies and leaves it behind.
fn clamp_timeout(options: &mut serde_json::Map<String, Value>, left: Duration) {
    let left = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    let asked = match options.get("timeout") {
        None => RUN_TIMEOUT,
        Some(value) => match value.as_u64() {
            Some(secs) => secs,
            None => return,
        },
    };
    options.insert("timeout".to_owned(), json!(asked.min(left.max(1))));
}

/// Parses the call as the command line `acus NAME FLAGS -- POSITIONALS`, with the command's own
/// clap definition.
fn parse(name: &str, call: &Call, skip: &[&str]) -> Result<Cmd, HostError> {
    let mut words = vec!["acus".to_owned(), name.to_owned()];
    words.extend(call.argv(name, skip)?);
    Cli::try_parse_from(&words)
        .map(|cli| cli.cmd)
        .map_err(|e| host_error(&crate::parse_error(&e, &words)))
}

fn host_error(error: &anyhow::Error) -> HostError {
    let failure = HostError::new(format!("{error:#}"));
    match find_hint(error) {
        Some(hint) => failure.with_hint(hint),
        None => failure,
    }
}

fn internal(name: &str) -> HostError {
    HostError::new(format!("{name}() is not available in scripts"))
}
#[cfg(test)]
mod tests {
    use super::clamp_timeout;
    use serde_json::{Value, json};
    use std::time::Duration;

    fn clamped(options: Value, left: Duration) -> Value {
        let Value::Object(mut options) = options else {
            panic!("options are an object");
        };
        clamp_timeout(&mut options, left);
        Value::Object(options)
    }

    #[test]
    fn run_timeouts_are_cut_to_the_seconds_left_before_the_deadline() {
        let cases = [
            (json!({}), Duration::from_secs(500), json!({"timeout": 120})),
            (
                json!({}),
                Duration::from_millis(30_400),
                json!({"timeout": 31}),
            ),
            (
                json!({"timeout": 10}),
                Duration::from_secs(60),
                json!({"timeout": 10}),
            ),
            (
                json!({"timeout": 1000}),
                Duration::from_secs(60),
                json!({"timeout": 60}),
            ),
            (json!({}), Duration::ZERO, json!({"timeout": 1})),
            (
                json!({"timeout": "x"}),
                Duration::from_secs(5),
                json!({"timeout": "x"}),
            ),
        ];
        for (options, left, expected) in cases {
            assert_eq!(
                clamped(options.clone(), left),
                expected,
                "{options} {left:?}"
            );
        }
    }
}

// SPDX-License-Identifier: MPL-2.0
//! `pons self doctor`: checks the install, the Zed setup, the running proxies and the wire
//! log. It only reads; a missing piece is a failed check, not an error.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::log::Entry;
use serde_json::Value;

use crate::launcher::{self, LAUNCHERS, PACKAGE};
use crate::log::WireFields;
use capsa_core::pkg;
use capsa_core::which::{self, Env, Fs, Platform, RealFs};

mod adapter;
mod process;
mod render;
#[cfg(test)]
mod tests;
mod view;
mod zed;

use adapter::*;
use process::*;
use render::*;
use view::View;
use zed::*;

/// How many of pons' own log notes the report shows.
const LAST_NOTES: usize = 5;

/// Counts and recent notes from a pons wire log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LogSummary {
    pub lines: usize,
    /// The file is in the text format of older versions, so no line was counted.
    pub legacy_text: bool,
    pub cancels_seen: usize,
    pub cancels_dropped: usize,
    pub stop_escapes: usize,
    pub subagents_tracked: usize,
    pub subagents_finished: usize,
    /// Finishes by silence only, which a later sign of life can contradict.
    pub subagents_provisional: usize,
    pub fail_safe_events: usize,
    pub notifications_dropped: usize,
    pub stepped_aside: bool,
    /// The last "exiting:" note: why a pons process ended.
    pub last_exit: Option<String>,
    pub last_notes: Vec<String>,
}

impl LogSummary {
    pub fn cancels_forwarded(&self) -> usize {
        self.cancels_seen.saturating_sub(self.cancels_dropped)
    }
}

/// Summarizes a wire log with one JSON object per line, as `capsa_core::log` writes it.
pub fn summarize_log(text: &str) -> LogSummary {
    let mut s = LogSummary::default();
    let mut notes: VecDeque<String> = VecDeque::new();
    for entry in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Entry<WireFields>>(line).ok())
    {
        let WireFields { dir, text: rest } = entry.fields;
        let rest = rest.as_str();
        s.lines += 1;
        match dir.as_str() {
            "client->agent" => {
                if rest.contains("\"method\":\"session/cancel\"") || rest == "method=session/cancel"
                {
                    s.cancels_seen += 1;
                }
            }
            "pons" => {
                // A cancel never forwarded: dropped for a message (or by the
                // pre-0.4 filter), or held and replaced by a stop escape.
                if rest.starts_with("cancel filter fired")
                    || rest.starts_with("message sent while subagents run")
                {
                    s.cancels_dropped += 1;
                } else if rest.starts_with("stop escape") {
                    s.stop_escapes += 1;
                    s.cancels_dropped += 1;
                } else if rest.ends_with("launched, card kept open") {
                    s.subagents_tracked += 1;
                } else if rest.starts_with("background subagent") && rest.contains(" finished (") {
                    s.subagents_finished += 1;
                    if rest.ends_with(", provisional)") {
                        s.subagents_provisional += 1;
                    }
                } else if rest.starts_with("fail-safe") {
                    s.fail_safe_events += 1;
                } else if rest.starts_with("dropped a task-notification") {
                    s.notifications_dropped += 1;
                } else if rest.contains("stepping aside") {
                    s.stepped_aside = true;
                } else if rest.starts_with("exiting:") {
                    s.last_exit = Some(rest.to_owned());
                }
                if notes.len() == LAST_NOTES {
                    notes.pop_front();
                }
                notes.push_back(rest.to_owned());
            }
            _ => {}
        }
    }
    s.last_notes = notes.into();
    s.legacy_text = s.lines == 0 && !text.trim().is_empty();
    s
}

/// Everything `run` reports on, gathered by the caller.
pub struct Inputs {
    pub env: Env,
    pub agent_cmd: Vec<String>,
    pub log: Option<PathBuf>,
}

/// Answers nothing.
#[cfg(test)]
pub struct NoSys;
#[cfg(test)]
impl Sys for NoSys {}

struct RealSys;

impl Sys for RealSys {
    fn canonicalize(&self, p: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(p).ok()
    }

    fn file_len(&self, p: &Path) -> Option<u64> {
        std::fs::metadata(p).ok().map(|m| m.len())
    }

    fn global_bin_dir(&self, manager: pkg::Manager) -> Option<PathBuf> {
        pkg::global_bin_dir(manager)
    }

    #[cfg(unix)]
    fn pons_processes(&self) -> Vec<Proc> {
        let Ok(out) = std::process::Command::new("ps")
            .args(["-axo", "pid=,etime=,command="])
            .output()
        else {
            return Vec::new();
        };
        parse_ps(&String::from_utf8_lossy(&out.stdout), std::process::id())
    }

    fn exe_age_secs(&self) -> Option<u64> {
        let exe = std::env::current_exe().ok()?;
        let modified = std::fs::metadata(exe).ok()?.modified().ok()?;
        modified.elapsed().ok().map(|d| d.as_secs())
    }
}

/// The facts of one doctor run, before they are written out.
pub struct Report {
    pub version: &'static str,
    pub platform: &'static str,
    pub arch: &'static str,
    pub explicit_command: bool,
    pub would_run: Result<(String, String), String>,
    pub candidates: Vec<(String, Option<String>)>,
    pub searched_dirs: usize,
    /// Whether the adapter can be started through the launcher's discovery.
    pub adapter: AdapterCheck,
    pub settings_path: Option<String>,
    /// `None`: not found or unreadable.
    pub settings: Option<Settings>,
    /// Version and directory of the adapter Zed installed from its registry.
    pub registry_adapter: Option<(Option<String>, String)>,
    pub log: Option<LogReport>,
    pub processes: Vec<Proc>,
    pub exe_age_secs: Option<u64>,
}

pub struct LogReport {
    pub path: String,
    /// `None`: not found or unreadable.
    pub summary: Option<LogSummary>,
    pub size: Option<u64>,
    pub rotated_size: Option<u64>,
}

/// Gathers the facts. `fs` answers the launcher questions; `read` reads a
/// text file (settings, manifest, log); `sys` the rest.
pub fn gather(
    inputs: &Inputs,
    fs: &dyn Fs,
    read: &dyn Fn(&Path) -> Option<String>,
    sys: &dyn Sys,
) -> Report {
    let env = &inputs.env;
    let plan = launcher::plan(env, fs, &inputs.agent_cmd);
    let configured_adapter = plan.as_ref().ok().and_then(|spawn| {
        configured_adapter(Path::new(&spawn.program), &spawn.args, sys, read)
            .map(|(v, p)| (v, p.display().to_string()))
    });
    let would_run = plan.map(|spawn| {
        let cmd = format!(
            "{} {}",
            Path::new(&spawn.program).display(),
            spawn.args.join(" ")
        );
        (spawn.how, cmd)
    });
    let candidates = LAUNCHERS
        .iter()
        .map(|(name, args)| {
            (
                format!("{name} {}", args.join(" ")),
                which::find_in(env, fs, name, None).map(|p| p.display().to_string()),
            )
        })
        .collect();
    let detected = launcher::detect(env, fs).map(|(i, p)| (LAUNCHERS[i].0, p));
    let adapter = adapter_check(
        !inputs.agent_cmd.is_empty(),
        detected.as_ref().map(|(name, p)| (*name, p.as_path())),
        configured_adapter.as_ref().map(|(v, _)| v.as_str()),
        || install_hint(env, fs, sys),
    );
    let settings_path = settings_path(env);
    let settings = settings_path
        .as_ref()
        .and_then(|p| read(p))
        .map(|text| find_pons_servers(&text));
    let registry_adapter =
        adapter_dir(env).map(|dir| (adapter_version(&dir, read), dir.display().to_string()));
    let log = inputs.log.as_ref().map(|path| {
        let rotated = capsa_core::log::rotated(path);
        LogReport {
            path: path.display().to_string(),
            summary: read(path).map(|t| summarize_log(&t)),
            size: sys.file_len(path),
            rotated_size: sys.file_len(&rotated),
        }
    });
    Report {
        version: env!("CARGO_PKG_VERSION"),
        platform: env.platform.name(),
        arch: std::env::consts::ARCH,
        explicit_command: !inputs.agent_cmd.is_empty(),
        would_run,
        candidates,
        searched_dirs: which::search_dirs(env, fs).len(),
        adapter,
        settings_path: settings_path.map(|p| p.display().to_string()),
        settings,
        registry_adapter,
        log,
        processes: sys.pons_processes(),
        exe_age_secs: sys.exe_age_secs(),
    }
}

/// Picks the log to summarize: `--log`/`PONS_LOG`, else the newest per-start log in
/// the log directory, else `pons.log` in the temp dir if it exists.
pub fn pick_log(given: Option<PathBuf>, log_dir: Option<&Path>) -> Option<PathBuf> {
    given
        .or_else(|| crate::log::newest_log(log_dir))
        .or_else(|| Some(std::env::temp_dir().join("pons.log")).filter(|p| p.is_file()))
}

/// Gathers the real environment and returns the report, written in `format`. The outcome
/// is [`Outcome::Empty`] (exit 1) when a check fails.
pub fn run(
    agent_cmd: Vec<String>,
    log: Option<PathBuf>,
    log_dir: Option<&Path>,
    format: Format,
) -> anyhow::Result<Outcome> {
    let inputs = Inputs {
        env: Env::from_process(),
        agent_cmd,
        log: pick_log(log, log_dir),
    };
    let read = |p: &Path| {
        std::fs::read(p)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    let report = gather(&inputs, &RealFs, &read, &RealSys);
    capsa_core::render::emit(&View::new(report), format)
}

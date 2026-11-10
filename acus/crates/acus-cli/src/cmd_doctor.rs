// SPDX-License-Identifier: MPL-2.0
//! `acus self doctor`: checks the install, config, skill, hooks and guard files. Exit 1 when a
//! check fails; warnings do not change the exit code.

use crate::cmd_hook;
use crate::cmd_setup::HOOKS;
use crate::cmd_skill::SKILL_DEF;
use crate::config::{self, Config};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hooks::{Agent, HookFile, HookSpec, HookState};
use capsa_core::render::{Check, Doctor, Group, Status, emit};
use capsa_core::skill::{self, SkillState, Target};
use serde_json::Value;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Tools that must be matched by the PreToolUse guard hook.
const GUARDED: [&str; 5] = ["Bash", "Read", "Grep", "Glob", "PowerShell"];

/// The guard's log rotates at this size, so a larger file is not expected.
const LOG_ROTATE_BYTES: u64 = 4 * 1024 * 1024;

pub fn run(format: Format) -> Result<Outcome> {
    let home = std::env::home_dir().context("no home directory")?;
    let (cfg, problem) = config::load_or_default();
    let settings = Agent::ClaudeCode.path();
    let groups = vec![
        install_group(),
        config_group(&cfg, problem.as_deref()),
        skill_group(&home, &settings),
        hooks_group(&settings),
        guard_group(&cfg),
    ];
    emit(
        &Doctor::new("acus", env!("CARGO_PKG_VERSION"), groups),
        format,
    )
}

fn check(
    name: &'static str,
    status: Status,
    detail: impl Into<String>,
    hint: Option<&str>,
) -> Check {
    Check {
        name: Cow::Borrowed(name),
        status,
        detail: detail.into(),
        hint: hint.map(str::to_owned),
    }
}

/// The same file, resolved through symlinks where it can be.
fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    canon(a) == canon(b)
}

fn install_group() -> Group {
    let mut checks = vec![check(
        "version",
        Status::Ok,
        format!("acus {}", env!("CARGO_PKG_VERSION")),
        None,
    )];
    let exe = std::env::current_exe().ok();
    match &exe {
        Some(exe) => checks.push(check("binary", Status::Ok, exe.display().to_string(), None)),
        None => checks.push(check(
            "binary",
            Status::Warn,
            "cannot tell which binary runs",
            None,
        )),
    }
    match (capsa_core::which::find("acus"), &exe) {
        (Some(on_path), Some(exe)) if !same_file(&on_path, exe) => checks.push(check(
            "PATH",
            Status::Warn,
            format!("acus on PATH is {}", on_path.display()),
            Some("the hooks run the acus on PATH; remove the other copy or update it"),
        )),
        (Some(on_path), _) => checks.push(check(
            "PATH",
            Status::Ok,
            on_path.display().to_string(),
            None,
        )),
        (None, _) => checks.push(check(
            "PATH",
            Status::Warn,
            "acus is not on PATH",
            Some("the hooks run bare `acus`; add the directory of this binary to PATH"),
        )),
    }
    Group {
        name: "install",
        checks,
    }
}

fn config_group(cfg: &Config, problem: Option<&str>) -> Group {
    let file = match problem {
        Some(message) => check(
            "config file",
            Status::Fail,
            message,
            Some("fix or delete the config file; the message names the line"),
        ),
        None if cfg.path.exists() => check(
            "config file",
            Status::Ok,
            cfg.path.display().to_string(),
            None,
        ),
        None => check(
            "config file",
            Status::Ok,
            format!("{} (not created; defaults apply)", cfg.path.display()),
            None,
        ),
    };
    let disabled = if cfg.commands.disabled.is_empty() {
        check("disabled commands", Status::Ok, "none", None)
    } else {
        check(
            "disabled commands",
            Status::Ok,
            cfg.commands.disabled.join(", "),
            None,
        )
    };
    Group {
        name: "config",
        checks: vec![file, disabled],
    }
}

/// The skill of one agent, reported only when that agent's home exists, so a missing skill for
/// an agent the user does not run is no warning.
fn skill_check(home: &Path, target: Target, name: &'static str, agent_home: &Path) -> Check {
    let state = skill::state(home, &SKILL_DEF, target);
    let dirs = target
        .dirs(home, SKILL_DEF.tool)
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let present = agent_home.is_dir();
    match state {
        SkillState::Current => check(name, Status::Ok, format!("current in {dirs}"), None),
        SkillState::Outdated => check(
            name,
            Status::Warn,
            format!("outdated in {dirs}"),
            Some("run acus self setup"),
        ),
        SkillState::Missing if present => check(
            name,
            Status::Warn,
            format!("not installed in {dirs}"),
            Some("run acus self setup"),
        ),
        SkillState::Missing => check(
            name,
            Status::Ok,
            format!("not installed; {} does not exist", agent_home.display()),
            None,
        ),
    }
}

fn skill_group(home: &Path, settings: &Path) -> Group {
    let mut checks = Vec::new();
    // The settings file sits in Claude Code's config dir, which is the agent home for Claude.
    if let Some(claude_home) = settings.parent() {
        checks.push(skill_check(
            home,
            Target::Claude,
            "claude skill",
            claude_home,
        ));
    }
    let codex_home =
        std::env::var_os("CODEX_HOME").map_or_else(|| home.join(".codex"), PathBuf::from);
    if codex_home.is_dir() {
        checks.push(skill_check(home, Target::Codex, "codex skill", &codex_home));
    }
    Group {
        name: "skill",
        checks,
    }
}

fn hooks_group(settings: &Path) -> Group {
    let file = match HookFile::open_at(Agent::ClaudeCode, settings) {
        Ok(file) => file,
        Err(e) => {
            return Group {
                name: "hooks",
                checks: vec![check(
                    "settings",
                    Status::Fail,
                    e.to_string(),
                    Some("fix or delete the settings file; the message names the problem"),
                )],
            };
        }
    };
    Group {
        name: "hooks",
        checks: HOOKS
            .iter()
            .map(|spec| hook_check(spec, &file.state(spec)))
            .collect(),
    }
}

/// The check for one hook. A PreToolUse matcher that leaves a guarded tool out warns, because the
/// guard never sees that tool.
fn hook_check(spec: &HookSpec, state: &HookState) -> Check {
    let matcher = match state {
        HookState::Installed => Some(spec.matcher.unwrap_or("")),
        HookState::CustomMatcher { matcher } => Some(matcher.as_deref().unwrap_or("")),
        HookState::Outdated { .. } | HookState::Missing => None,
    };
    if let (Some(matcher), "PreToolUse") = (matcher, spec.event) {
        let missing: Vec<&str> = GUARDED
            .into_iter()
            .filter(|tool| !covers(matcher, tool))
            .collect();
        if !missing.is_empty() {
            return check(
                "PreToolUse hook",
                Status::Warn,
                format!("matcher {matcher:?} does not cover {}", missing.join(", ")),
                Some("run acus self setup"),
            );
        }
    }
    state.check(spec, "acus self setup")
}

/// Whether a `|`-separated matcher reaches `tool`. An empty matcher or `*` reaches every tool.
fn covers(matcher: &str, tool: &str) -> bool {
    matcher.is_empty() || matcher == "*" || matcher.split('|').any(|m| m == tool)
}

fn guard_group(cfg: &Config) -> Group {
    let guard = cmd_hook::guard_config(cfg);
    let log_dir = guard
        .log_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let mut checks = vec![dir_check("guard log dir", &log_dir)];
    checks.push(log_size_check(&guard.log_path));
    checks.push(log_age_check(&guard.log_path));
    checks.push(dir_check("guard hints dir", &guard.state_dir));
    Group {
        name: "guard",
        checks,
    }
}

/// Whether `dir` takes a file: an existing directory is probed, a missing one is created on
/// first use and so is not an error.
fn dir_check(name: &'static str, dir: &Path) -> Check {
    if !dir.is_dir() {
        return check(
            name,
            Status::Ok,
            format!("{} not created yet", dir.display()),
            None,
        );
    }
    match writable(dir) {
        Ok(()) => check(
            name,
            Status::Ok,
            format!("{} writable", dir.display()),
            None,
        ),
        Err(e) => check(
            name,
            Status::Fail,
            format!("{} not writable: {e}", dir.display()),
            Some("make the directory writable, or point ACUS_CONFIG at another directory"),
        ),
    }
}

fn writable(dir: &Path) -> std::io::Result<()> {
    let probe = dir.join(format!(".acus-doctor-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    std::fs::remove_file(&probe)
}

fn log_size_check(log: &Path) -> Check {
    match std::fs::metadata(log) {
        Ok(meta) => {
            let status = if meta.len() > LOG_ROTATE_BYTES {
                Status::Warn
            } else {
                Status::Ok
            };
            check(
                "guard log size",
                status,
                format!("{} KiB", meta.len() / 1024),
                (status == Status::Warn)
                    .then_some("the log should rotate at 4 MiB; check the guard runs"),
            )
        }
        Err(_) => check("guard log size", Status::Ok, "no log yet", None),
    }
}

fn log_age_check(log: &Path) -> Check {
    let last = std::fs::read_to_string(log).ok().and_then(|text| {
        let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
        let entry: Value = serde_json::from_str(line).ok()?;
        entry["ts"].as_u64()
    });
    match last {
        Some(ts) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            check(
                "last guard entry",
                Status::Ok,
                format!("{} ago", ago(now.saturating_sub(ts))),
                None,
            )
        }
        None => check("last guard entry", Status::Ok, "no entries yet", None),
    }
}

/// A coarse age: minutes, hours or days.
fn ago(secs: u64) -> String {
    match secs {
        0..3600 => format!("{} min", secs / 60),
        3600..86_400 => format!("{} h", secs / 3600),
        _ => format!("{} d", secs / 86_400),
    }
}

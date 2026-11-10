// SPDX-License-Identifier: MPL-2.0
//! `census self doctor`: checks the install, config, skill, hooks and state files. Exit 1 when a
//! check fails; warnings do not change the exit code.

use crate::cmd_self::SKILL;
use crate::out::Out;
use anyhow::{Context, Result};
use capsa_core::claude_settings::runs;
use capsa_core::exit::Outcome;
use capsa_core::hooks::{Agent, HookFile};
use capsa_core::log;
use capsa_core::render::{Check, Doctor, Group, Status, emit};
use capsa_core::skill::{self, SkillState, Target};
use census::config::Config;
use census::hook::{self, ErrorLine};
use serde_json::Value;
use std::borrow::Cow;
use std::path::Path;

use crate::setup::{HOOKS, TOOL};

pub fn run(out: &Out) -> Result<Outcome> {
    let home = std::env::home_dir().context("no home directory")?;
    let settings = Agent::ClaudeCode.path();
    let groups = vec![
        install_group(),
        config_group(),
        skill_group(&home, &settings),
        hooks_group(&settings),
        state_group(),
    ];
    emit(
        &Doctor::new(TOOL, env!("CARGO_PKG_VERSION"), groups),
        out.format,
    )
}

fn check(
    name: impl Into<Cow<'static, str>>,
    status: Status,
    detail: impl Into<String>,
    hint: Option<&str>,
) -> Check {
    Check {
        name: name.into(),
        status,
        detail: detail.into(),
        hint: hint.map(str::to_owned),
    }
}

fn install_group() -> Group {
    let exe = std::env::current_exe().ok();
    let binary = match &exe {
        Some(exe) => check("binary", Status::Ok, exe.display().to_string(), None),
        None => check(
            "binary",
            Status::Warn,
            "cannot tell which binary runs",
            None,
        ),
    };
    let on_path = match (capsa_core::which::find(TOOL), &exe) {
        (Some(on_path), Some(exe)) if !same_file(&on_path, exe) => check(
            "PATH",
            Status::Warn,
            format!("census on PATH is {}", on_path.display()),
            Some("the hooks run the census on PATH; remove the other copy or update it"),
        ),
        (Some(on_path), _) => check("PATH", Status::Ok, on_path.display().to_string(), None),
        (None, _) => check(
            "PATH",
            Status::Warn,
            "census is not on PATH",
            Some("the hooks run bare `census`; add the directory of this binary to PATH"),
        ),
    };
    Group {
        name: "install",
        checks: vec![
            check(
                "version",
                Status::Ok,
                format!("census {}", env!("CARGO_PKG_VERSION")),
                None,
            ),
            binary,
            on_path,
        ],
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    canon(a) == canon(b)
}

fn config_group() -> Group {
    let path = Config::path();
    let file = match Config::load_from(&path) {
        Ok(_) if path.exists() => {
            check("config file", Status::Ok, path.display().to_string(), None)
        }
        Ok(_) => check(
            "config file",
            Status::Ok,
            format!("{} (not created; defaults apply)", path.display()),
            None,
        ),
        Err(e) => check(
            "config file",
            Status::Fail,
            e.to_string(),
            Some("fix or delete the config file; the message names the file"),
        ),
    };
    Group {
        name: "config",
        checks: vec![file],
    }
}

fn skill_group(home: &Path, settings: &Path) -> Group {
    let mut checks = Vec::new();
    // The settings file sits in Claude Code's config dir, which is the agent home for Claude.
    if let Some(claude_home) = settings.parent() {
        checks.push(skill_check(home, claude_home));
    }
    Group {
        name: "skill",
        checks,
    }
}

/// The skill, reported only when Claude Code's home exists, so a missing skill for an agent the
/// user does not run is no warning.
fn skill_check(home: &Path, claude_home: &Path) -> Check {
    let dirs = Target::Claude
        .dirs(home, TOOL)
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    match skill::state(home, &SKILL, Target::Claude) {
        SkillState::Current => check(
            "claude skill",
            Status::Ok,
            format!("current in {dirs}"),
            None,
        ),
        SkillState::Outdated => check(
            "claude skill",
            Status::Warn,
            format!("outdated in {dirs}"),
            Some("run census self setup"),
        ),
        SkillState::Missing if claude_home.is_dir() => check(
            "claude skill",
            Status::Warn,
            format!("not installed in {dirs}"),
            Some("run census self setup"),
        ),
        SkillState::Missing => check(
            "claude skill",
            Status::Ok,
            format!("not installed; {} does not exist", claude_home.display()),
            None,
        ),
    }
}

fn hooks_group(settings: &Path) -> Group {
    let file = match HookFile::open_at(Agent::ClaudeCode, settings) {
        Ok(read) => read,
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
    let command = file
        .value()
        .get("statusLine")
        .and_then(|line| line.get("command"))
        .and_then(Value::as_str);
    let mut checks = vec![match command {
        Some(c) if runs(c, TOOL, "statusline") => {
            check("statusLine", Status::Ok, "census statusline", None)
        }
        _ => check(
            "statusLine",
            Status::Warn,
            "not registered",
            Some("run census self setup"),
        ),
    }];
    checks.extend(
        HOOKS
            .iter()
            .map(|spec| file.state(spec).check(spec, "census self setup")),
    );
    Group {
        name: "hooks",
        checks,
    }
}

fn state_group() -> Group {
    Group {
        name: "state",
        checks: vec![
            state_dir_check(&capsa_core::paths::state_dir(TOOL)),
            hook_log_check(&hook::error_log()),
        ],
    }
}

/// Whether `dir` takes a file: an existing directory is probed, a missing one is created on first
/// use and so is not an error.
fn state_dir_check(dir: &Path) -> Check {
    if !dir.is_dir() {
        return check(
            "state dir",
            Status::Ok,
            format!("{} not created yet", dir.display()),
            None,
        );
    }
    match writable(dir) {
        Ok(()) => check(
            "state dir",
            Status::Ok,
            format!("{} writable", dir.display()),
            None,
        ),
        Err(e) => check(
            "state dir",
            Status::Fail,
            format!("{} is not writable: {e}", dir.display()),
            Some("fix the permissions on the state directory"),
        ),
    }
}

fn writable(dir: &Path) -> std::io::Result<()> {
    let probe = dir.join(format!(".census-doctor-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)?;
    std::fs::remove_file(&probe)
}

/// Reports the errors the hook has logged, the newest last. Past errors warn; they do not fail.
fn hook_log_check(file: &Path) -> Check {
    let entries = log::read::<ErrorLine>(file);
    let Some(last) = entries.last() else {
        return check("hook log", Status::Ok, "no errors", None);
    };
    check(
        "hook log",
        Status::Warn,
        format!(
            "{} error(s); last at {}: {}",
            entries.len(),
            last.ts,
            last.fields.message
        ),
        Some("fix the cause; the hook logs its errors to hook.jsonl in the state directory"),
    )
}

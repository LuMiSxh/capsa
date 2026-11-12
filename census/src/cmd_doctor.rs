// SPDX-License-Identifier: MPL-2.0
//! `census self doctor`: checks the install, config, skill, hooks and state files. Exit 1 when a
//! check fails; warnings do not change the exit code.

use crate::cmd_self::SKILL;
use crate::out::Out;
use anyhow::{Context, Result};
use capsa_core::config::legacy_json;
use capsa_core::doctor::{check, dir_check, log_checks, ok, path_check};
use capsa_core::exit::Outcome;
use capsa_core::hooks::{Agent, HookFile, StatusLineState};
use capsa_core::log;
use capsa_core::render::{Check, Doctor, Group, Status, emit};
use capsa_core::skill::{self, SkillState, Target};
use census::config::Config;
use census::hook::{self, ErrorLine};
use census::usage::{PRICE_LOG, PRICE_LOG_LIMIT};
use std::path::Path;

use crate::setup::{HOOKS, STATUS_LINE, TOOL};

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

fn install_group() -> Group {
    let exe = std::env::current_exe().ok();
    let binary = match &exe {
        Some(exe) => ok("binary", exe.display().to_string()),
        None => check(
            "binary",
            Status::Warn,
            "cannot tell which binary runs",
            None,
        ),
    };
    let on_path = path_check(
        TOOL,
        capsa_core::which::find(TOOL).as_deref(),
        exe.as_deref(),
    );
    Group {
        name: "install",
        checks: vec![
            ok("version", format!("census {}", env!("CARGO_PKG_VERSION"))),
            binary,
            on_path,
        ],
    }
}

fn config_group() -> Group {
    let file = Config::file();
    let path = file.path().display().to_string();
    let mut checks = vec![match file.load::<Config>() {
        Ok(_) if file.path().exists() => ok("config file", path.as_str()),
        Ok(_) => ok(
            "config file",
            format!("{path} (not created; defaults apply)"),
        ),
        Err(e) => check(
            "config file",
            Status::Fail,
            e.to_string(),
            Some("fix or delete the config file; the message names the file"),
        ),
    }];
    if let Some(legacy) = legacy_json(TOOL) {
        let hint =
            format!("set the settings again in {path} with `census self config set <key> <value>`");
        checks.push(check(
            "legacy config",
            Status::Warn,
            format!("{} is no longer read", legacy.display()),
            Some(hint.as_str()),
        ));
    }
    Group {
        name: "config",
        checks,
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
    let mut checks = vec![match file.status_line(&STATUS_LINE) {
        StatusLineState::Installed | StatusLineState::Outdated { .. } => {
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
    let mut checks = vec![dir_check(
        "state dir",
        &capsa_core::paths::state_dir(TOOL),
        "fix the permissions on the state directory",
    )];
    match hook::error_log() {
        Some(file) => {
            checks.extend(log_checks("hook", &file, hook::LOG_LIMIT));
            checks.push(hook_error_check(&file));
        }
        None => checks.push(check(
            "hook log",
            Status::Warn,
            "no state directory is known",
            Some("set HOME or XDG_STATE_HOME"),
        )),
    }
    if let Some(file) = capsa_core::paths::log_file(TOOL, PRICE_LOG) {
        checks.extend(log_checks("prices", &file, PRICE_LOG_LIMIT));
    }
    Group {
        name: "state",
        checks,
    }
}

/// Reports the errors the hook has logged, the newest last. Past errors warn; they do not fail.
fn hook_error_check(file: &Path) -> Check {
    let entries = match log::read::<ErrorLine>(file) {
        Ok(entries) => entries,
        Err(e) => return check("hook errors", Status::Warn, e.to_string(), None),
    };
    let Some(last) = entries.last() else {
        return ok("hook errors", "no errors");
    };
    check(
        "hook errors",
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

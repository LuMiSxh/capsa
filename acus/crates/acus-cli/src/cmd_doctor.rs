// SPDX-License-Identifier: MPL-2.0
//! `acus self doctor`: checks the install, config, skill, hooks and guard files. Exit 1 when a
//! check fails; warnings do not change the exit code.

use crate::cmd_hook;
use crate::cmd_setup::HOOKS;
use crate::cmd_skill::SKILL_DEF;
use crate::config::{self, Config};
use crate::guard_log;
use anyhow::{Context, Result};
use capsa_core::doctor::{
    check, dir_check, emit_report, log_checks, path_check, repair_hooks, repair_skill,
};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use capsa_core::hooks::{Agent, HookFile, HookSpec, HookState};
use capsa_core::render::{Check, Doctor, Group, Status};
use capsa_core::skill::{self, SkillState, Target, installed_target};
use std::path::{Path, PathBuf};

/// Tools that must be matched by the PreToolUse guard hook.
const GUARDED: [&str; 5] = ["Bash", "Read", "Grep", "Glob", "PowerShell"];

#[derive(clap::Args)]
pub struct Args {
    /// Repairs what is safe to repair first: updates an outdated skill and adds or updates hooks.
    /// Deletions, config values, guard logs from older releases and network calls keep their hints.
    #[arg(long)]
    pub fix: bool,
}

pub fn run(format: Format, fix: bool) -> Result<Outcome> {
    let home = std::env::home_dir().context("no home directory")?;
    let (cfg, problem) = config::load_or_default();
    let settings = Agent::ClaudeCode.path();
    let fixed = fix.then(|| repair(&home, &settings)).transpose()?;
    let groups = vec![
        install_group(),
        config_group(&cfg, problem.as_deref()),
        skill_group(&home, &settings),
        hooks_group(&settings),
        guard_group(&cfg),
    ];
    emit_report(
        &Doctor::new("acus", env!("CARGO_PKG_VERSION"), groups),
        fixed,
        format,
    )
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
    checks.push(path_check(
        "acus",
        capsa_core::which::find("acus").as_deref(),
        exe.as_deref(),
    ));
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
    let dirs = skill_dirs(home, target);
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
    let codex_home = codex_dir(home);
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
    const DIR_HINT: &str = "make the directory writable, or point ACUS_CONFIG at another directory";
    let guard = cmd_hook::guard_config(cfg);
    let log_dir = guard
        .log_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let mut checks = vec![dir_check("guard log dir", &log_dir, DIR_HINT)];
    checks.extend(log_checks(
        "guard",
        &guard.log_path,
        acus_guard::log::MAX_BYTES,
    ));
    checks.extend(leftover_check(&cfg.path));
    checks.push(dir_check("guard hints dir", &guard.state_dir, DIR_HINT));
    Group {
        name: "guard",
        checks,
    }
}

/// Warns about guard log files that older releases left beside the config file. `self setup`
/// moves or merges them.
fn leftover_check(config: &Path) -> Option<Check> {
    let files = guard_log::leftovers(config);
    (!files.is_empty()).then(|| {
        let names: Vec<_> = files
            .iter()
            .map(|file| file.display().to_string())
            .collect();
        check(
            "guard log location",
            Status::Warn,
            format!("{} left from before the move", names.join(", ")),
            Some("run acus self setup"),
        )
    })
}

/// The directories a skill of `target` is installed in, for the report.
fn skill_dirs(home: &Path, target: Target) -> String {
    target
        .dirs(home, SKILL_DEF.tool)
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Codex's home: `CODEX_HOME` when set, else `~/.codex`.
fn codex_dir(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME").map_or_else(|| home.join(".codex"), PathBuf::from)
}

/// Makes the repairs `--fix` names and returns one check per repair made. A check that needs a
/// choice, a deletion or a changed config value keeps its hint.
fn repair(home: &Path, settings: &Path) -> Result<Vec<Check>> {
    let mut fixed = repair_skills(home)?;
    fixed.extend(repair_settings(home, settings)?);
    Ok(fixed)
}

/// Updates an outdated skill for Claude Code and for Codex; a missing skill keeps its hint.
fn repair_skills(home: &Path) -> Result<Vec<Check>> {
    let mut fixed = Vec::new();
    fixed.extend(repair_skill(
        home,
        &SKILL_DEF,
        Target::Claude,
        "claude skill",
    )?);
    fixed.extend(repair_skill(
        home,
        &SKILL_DEF,
        Target::Codex,
        "codex skill",
    )?);
    Ok(fixed)
}

/// Adds the missing acus hooks and replaces the outdated ones in an existing Claude Code setup.
/// A settings file that does not parse is left to the hooks group to report.
fn repair_settings(home: &Path, settings: &Path) -> Result<Vec<Check>> {
    let set_up = matches!(
        installed_target(home, SKILL_DEF.tool),
        Some(Target::Claude | Target::All)
    );
    if !set_up || !settings.exists() {
        return Ok(Vec::new());
    }
    let Ok(mut file) = HookFile::open_at(Agent::ClaudeCode, settings) else {
        return Ok(Vec::new());
    };
    let checks = repair_hooks(&mut file, &HOOKS);
    if !checks.is_empty() {
        file.write().hinted()?;
    }
    Ok(checks)
}

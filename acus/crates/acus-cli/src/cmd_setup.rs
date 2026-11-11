// SPDX-License-Identifier: MPL-2.0
//! `acus self setup`: installs the skill and registers acus's hooks in Claude Code's settings.

use crate::cmd_skill::SKILL_DEF;
use crate::hinted;
use anyhow::{Context, Result};
use capsa_core::claude_settings::{Step, runs};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use capsa_core::hooks::{Agent, HookFile, HookSpec};
use capsa_core::render::{Check, Doc, Render, Status, emit};
use capsa_core::skill::{self, SkillState, Target, installed_target};
use serde::Serialize;
use std::borrow::Cow;

#[derive(clap::Args)]
pub struct Args {
    /// Which agent gets the skill. The hooks go to Claude Code's settings, so `codex` is refused.
    #[arg(long, value_enum, default_value_t = Target::Claude)]
    target: Target,
    /// Print what would change and change nothing.
    #[arg(long, conflicts_with = "add_missing")]
    dry_run: bool,
    /// Add only the hooks that are missing, keeping every acus entry already there. Writes
    /// nothing unless acus was set up for Claude Code. `self update` runs this.
    #[arg(long)]
    add_missing: bool,
}

const HOOK: &str = "acus hook 2>/dev/null || true";

/// The hooks acus registers. SubagentStart has no matcher, so an entry the user scoped with one,
/// such as `Explore|Plan`, stays.
pub const HOOKS: [HookSpec; 3] = [
    HookSpec {
        event: "SessionStart",
        matcher: Some("startup|clear|compact"),
        command: HOOK,
        owns: owns_session,
        legacy: Some(owns_old_skill),
    },
    HookSpec {
        event: "PreToolUse",
        matcher: Some("Bash|Read|Grep|Glob|PowerShell"),
        command: HOOK,
        owns: owns_tool,
        legacy: Some(owns_old_guard),
    },
    HookSpec {
        event: "SubagentStart",
        matcher: None,
        command: HOOK,
        owns: owns_hook,
        legacy: None,
    },
];

/// Claims the `acus hook` command.
fn owns_hook(command: &str) -> bool {
    runs(command, "acus", "hook")
}

/// Claims the old SessionStart hook, including the `acus skill` form of older releases.
fn owns_old_skill(command: &str) -> bool {
    runs(command, "acus", "self skill") || runs(command, "acus", "skill")
}

/// Claims the old PreToolUse hook.
fn owns_old_guard(command: &str) -> bool {
    runs(command, "acus", "guard")
}

/// Claims a SessionStart entry of acus, old or new.
fn owns_session(command: &str) -> bool {
    owns_hook(command) || owns_old_skill(command)
}

/// Claims a PreToolUse entry of acus, old or new.
fn owns_tool(command: &str) -> bool {
    owns_hook(command) || owns_old_guard(command)
}

/// The check for one hook step, naming the file it went to.
fn hook_step(event: &str, step: Step, dry_run: bool, file: &HookFile) -> Check {
    Check {
        name: Cow::Owned(format!("hook {event}")),
        status: Status::Ok,
        detail: format!("{} in {}", step.text(dry_run), file.path().display()),
        hint: None,
    }
}

/// Adds the hooks that are missing, for `self update`. It writes nothing when settings.json does
/// not exist or acus was never set up for Claude Code, and prints nothing when all hooks are there.
fn add_missing(format: Format) -> Result<Outcome> {
    let home = std::env::home_dir().context("no home directory")?;
    let set_up = matches!(
        installed_target(&home, SKILL_DEF.tool),
        Some(Target::Claude | Target::All)
    );
    if !set_up || !Agent::ClaudeCode.path().exists() {
        return Ok(Outcome::Found);
    }
    let mut file = HookFile::open(Agent::ClaudeCode).hinted()?;
    let steps: Vec<_> = HOOKS
        .iter()
        .map(|spec| (spec.event, file.add(spec)))
        .filter(|(_, step)| step.writes())
        .collect();
    let checks: Vec<Check> = steps
        .into_iter()
        .map(|(event, step)| hook_step(event, step, false, &file))
        .collect();
    if checks.is_empty() {
        return Ok(Outcome::Found);
    }
    file.write().hinted()?;
    emit(
        &Setup {
            dry_run: false,
            checks,
        },
        format,
    )
}

/// The steps of one setup run, one check per step.
#[derive(Debug, Serialize)]
pub struct Setup {
    pub dry_run: bool,
    pub checks: Vec<Check>,
}

impl Render for Setup {
    fn doc(&self, doc: &mut Doc) {
        doc.title("acus self setup");
        for check in &self.checks {
            doc.check(check);
        }
        if self.dry_run {
            doc.line("dry run: nothing was written");
        }
    }
}

pub fn run(a: &Args, format: Format) -> Result<Outcome> {
    if a.add_missing {
        return add_missing(format);
    }
    if a.target == Target::Codex {
        return Err(hinted(
            "setup registers Claude Code hooks",
            "use --target claude or --target all",
        ));
    }
    let home = std::env::home_dir().context("no home directory")?;
    let dirs = a
        .target
        .dirs(&home, SKILL_DEF.tool)
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let step = match skill::state(&home, &SKILL_DEF, a.target) {
        SkillState::Current => Step::Set,
        SkillState::Missing => Step::Install,
        SkillState::Outdated => Step::Replace,
    };
    let mut checks = vec![Check {
        name: Cow::Borrowed("skill"),
        status: Status::Ok,
        detail: format!("{} {dirs}", step.text(a.dry_run)),
        hint: None,
    }];
    if step.writes() && !a.dry_run {
        skill::install(&home, &SKILL_DEF, a.target).hinted()?;
    }

    let mut file = HookFile::open(Agent::ClaudeCode).hinted()?;
    let mut writes = false;
    for spec in &HOOKS {
        let step = file.set(spec);
        writes |= step.writes();
        checks.push(hook_step(spec.event, step, a.dry_run, &file));
    }
    if writes && !a.dry_run {
        file.write().hinted()?;
    }
    emit(
        &Setup {
            dry_run: a.dry_run,
            checks,
        },
        format,
    )
}

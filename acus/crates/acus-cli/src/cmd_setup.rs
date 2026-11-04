// SPDX-License-Identifier: MPL-2.0
//! `acus self setup`: installs the skill and registers acus's hooks in Claude Code's settings.

use crate::cmd_skill::SKILL_DEF;
use anyhow::{Context, Result};
use capsa_core::claude_settings::{self, Step, runs};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::{Hinted, ResultExt};
use capsa_core::render::{Check, Doc, Render, Status, emit};
use capsa_core::skill::{self, SkillState, Target};
use serde::Serialize;
use std::borrow::Cow;

#[derive(clap::Args)]
pub struct Args {
    /// Which agent gets the skill. The hooks go to Claude Code's settings, so `codex` is refused.
    #[arg(long, value_enum, default_value_t = Target::Claude)]
    target: Target,
    /// Print what would change and change nothing.
    #[arg(long)]
    dry_run: bool,
}

/// Tells whether a hook command is one that acus registered before.
type Owns = fn(&str) -> bool;

/// The hooks acus registers: event, matcher, command and the test that claims an older entry.
const HOOKS: [(&str, &str, &str, Owns); 2] = [
    (
        "SessionStart",
        "startup|clear|compact",
        "acus hook 2>/dev/null || true",
        owns_session,
    ),
    (
        "PreToolUse",
        "Bash|Read|Grep|Glob|PowerShell",
        "acus hook 2>/dev/null || true",
        owns_tool,
    ),
];

/// Claims the `acus hook` command.
pub fn owns_hook(command: &str) -> bool {
    runs(command, "acus", "hook")
}

/// Claims the old SessionStart hook, including the `acus skill` form of older releases.
pub fn owns_old_skill(command: &str) -> bool {
    runs(command, "acus", "self skill") || runs(command, "acus", "skill")
}

/// Claims the old PreToolUse hook.
pub fn owns_old_guard(command: &str) -> bool {
    runs(command, "acus", "guard")
}

/// Claims a SessionStart entry that setup replaces, old or new.
fn owns_session(command: &str) -> bool {
    owns_hook(command) || owns_old_skill(command)
}

/// Claims a PreToolUse entry that setup replaces, old or new.
fn owns_tool(command: &str) -> bool {
    owns_hook(command) || owns_old_guard(command)
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
    if a.target == Target::Codex {
        return Err(Hinted::new(
            "setup registers Claude Code hooks",
            "use --target claude or --target all",
        )
        .into());
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

    let mut settings = claude_settings::read(&claude_settings::path()).hinted()?;
    let mut writes = false;
    for (event, matcher, command, owns) in HOOKS {
        let step =
            claude_settings::set_hook(&mut settings.value, event, Some(matcher), command, owns);
        writes |= step.writes();
        checks.push(Check {
            name: Cow::Owned(format!("hook {event}")),
            status: Status::Ok,
            detail: format!("{} in {}", step.text(a.dry_run), settings.path.display()),
            hint: None,
        });
    }
    if writes && !a.dry_run {
        claude_settings::write(&settings).hinted()?;
    }
    emit(
        &Setup {
            dry_run: a.dry_run,
            checks,
        },
        format,
    )
}

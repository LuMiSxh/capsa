// SPDX-License-Identifier: MPL-2.0
//! `census self setup`: installs the skill, then adds census's statusLine and hooks to Claude Code's
//! `settings.json`. Entries in the bare form stay as they are; census's other forms are replaced.
//! The file is written only when
//! something changes, and the old one is copied to `settings.json.bak` first. The settings file and
//! the skill are handled by `capsa_core`; census only decides which entries it owns.

use crate::cmd_self::SKILL;
use crate::out::Out;
use anyhow::{Context, Result};
use capsa_core::claude_settings::{Step, runs};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use capsa_core::hooks::{Agent, HookFile, HookSpec, StatusLineSpec, StatusLineState};
use capsa_core::skill::{self, SkillState, Target};
use serde::Serialize;

/// The binary's name, as the commands in settings.json name it.
pub const TOOL: &str = "census";
/// The commands written into settings.json: bare names, which Claude Code resolves on PATH.
const HOOK: &str = "census hook";

/// Claims every `census hook` command.
fn owns_hook(command: &str) -> bool {
    runs(command, TOOL, "hook")
}

/// Claims every `census statusline` command, current and older.
fn owns_status_line(command: &str) -> bool {
    runs(command, TOOL, "statusline")
}

/// The statusLine that census registers.
pub const STATUS_LINE: StatusLineSpec = StatusLineSpec {
    command: "census statusline --line",
    owns: owns_status_line,
};

const fn hook(event: &'static str) -> HookSpec {
    HookSpec {
        event,
        matcher: None,
        command: HOOK,
        owns: owns_hook,
        legacy: None,
    }
}

/// The hooks census registers, in the order they are reported.
pub const HOOKS: [HookSpec; 3] = [
    hook("SessionStart"),
    hook("UserPromptSubmit"),
    hook("PostToolUse"),
];

#[derive(clap::Args)]
pub struct Args {
    /// Reports what would change and writes nothing, the skill included.
    #[arg(long)]
    dry_run: bool,
    /// Replaces a statusLine that census does not own.
    #[arg(long)]
    force: bool,
}

/// One line of the report.
#[derive(Serialize)]
struct Item {
    name: String,
    status: &'static str,
}

#[derive(Serialize)]
struct Report {
    dry_run: bool,
    settings: String,
    items: Vec<Item>,
}

/// Installs the skill and wires census into the settings, then prints what it did.
pub fn run(a: &Args, out: &Out) -> Result<Outcome> {
    let home = std::env::home_dir().context("no home directory")?;
    let mut file = HookFile::open(Agent::ClaudeCode).hinted()?;
    let before = file.value().clone();
    let skill_step = if skill::state(&home, &SKILL, Target::Claude) == SkillState::Current {
        Step::Set
    } else {
        Step::Install
    };
    let mut steps = vec![("skill".to_owned(), skill_step.text(a.dry_run))];
    steps.extend(wire(&mut file, a.force, a.dry_run));
    if !a.dry_run {
        if skill_step.writes() {
            skill::install(&home, &SKILL, Target::Claude).hinted()?;
        }
        if *file.value() != before {
            file.write().hinted()?;
        }
    }
    let report = Report {
        dry_run: a.dry_run,
        settings: file.path().display().to_string(),
        items: steps
            .into_iter()
            .map(|(name, status)| Item { name, status })
            .collect(),
    };
    print_report(&report, out)?;
    Ok(Outcome::Found)
}

/// Sets census's statusLine and hook for each event, returning the report text of each item.
fn wire(file: &mut HookFile, force: bool, dry_run: bool) -> Vec<(String, &'static str)> {
    let mut steps = vec![(
        "statusLine".to_owned(),
        wire_status_line(file, force, dry_run),
    )];
    for spec in &HOOKS {
        let step = file.set(spec);
        steps.push((format!("hook {}", spec.event), step.text(dry_run)));
    }
    steps
}

/// Sets census's statusLine and returns its report text. A statusLine that census does not own
/// stays unless `force` is set.
fn wire_status_line(file: &mut HookFile, force: bool, dry_run: bool) -> &'static str {
    let step = match file.status_line(&STATUS_LINE) {
        StatusLineState::Installed => return Step::Set.text(dry_run),
        StatusLineState::Missing => Step::Install,
        StatusLineState::Outdated { .. } => Step::Replace,
        StatusLineState::Foreign { .. } if force => {
            file.set_status_line(&STATUS_LINE);
            return if dry_run {
                "would install (replaces foreign statusLine)"
            } else {
                "installed (replaced foreign statusLine)"
            };
        }
        StatusLineState::Foreign { .. } => {
            return "skipped (foreign statusLine; --force replaces it)";
        }
    };
    file.set_status_line(&STATUS_LINE);
    step.text(dry_run)
}

/// Prints the report: one line per item, or the JSON document.
fn print_report(report: &Report, out: &Out) -> Result<()> {
    if out.format == Format::Json {
        println!("{}", serde_json::to_string(report)?);
        return Ok(());
    }
    for item in &report.items {
        println!("{}", out.field(&format!("{:<24}", item.name), item.status));
    }
    println!(
        "{}",
        out.field(&format!("{:<24}", "settings"), &report.settings)
    );
    if report.dry_run {
        println!("dry run: nothing was written");
    }
    Ok(())
}

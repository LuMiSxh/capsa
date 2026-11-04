// SPDX-License-Identifier: MPL-2.0
//! `census self setup`: installs the skill, then adds census's statusLine and hooks to Claude Code's
//! `settings.json`. Entries in the bare form stay as they are; census's other forms are replaced.
//! The file is written only when
//! something changes, and the old one is copied to `settings.json.bak` first. The settings file and
//! the skill are handled by `capsa_core`; census only decides which entries it owns.

use crate::cmd_self::SKILL;
use crate::out::Out;
use anyhow::{Context, Result};
use capsa_core::claude_settings::{self, Settings};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use capsa_core::skill::{self, SkillState, Target};
use serde::Serialize;
use serde_json::{Value, json};

/// The events that get census's hook, in the order they are reported.
const EVENTS: [&str; 3] = ["SessionStart", "UserPromptSubmit", "PostToolUse"];
/// The binary's name, as the commands in settings.json name it.
const TOOL: &str = "census";
/// The commands written into settings.json: bare names, which Claude Code resolves on PATH.
const HOOK: &str = "census hook";
const STATUS_LINE: &str = "census statusline --line";

#[derive(clap::Args)]
pub struct Args {
    /// Reports what would change and writes nothing, the skill included.
    #[arg(long)]
    dry_run: bool,
    /// Replaces a statusLine that census does not own.
    #[arg(long)]
    force: bool,
}

/// What setup does with an item.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The item is added.
    Install,
    /// census's own hook or statusLine runs in another form or with a stale command; it is replaced.
    Replace,
    /// A statusLine that census does not own is replaced, under `--force`.
    ReplaceForeign,
    /// The same entry is already there; nothing changes.
    Set,
    /// A statusLine that census does not own stays without `--force`.
    Skip,
}

impl Step {
    /// Whether this step writes the item.
    fn writes(self) -> bool {
        matches!(self, Self::Install | Self::Replace | Self::ReplaceForeign)
    }

    fn text(self, dry_run: bool) -> &'static str {
        match (self, dry_run) {
            (Self::Set, _) => "already set",
            (Self::Install, false) => "installed",
            (Self::Install, true) => "would install",
            (Self::Replace, false) => "replaced",
            (Self::Replace, true) => "would replace",
            (Self::ReplaceForeign, false) => "installed (replaced foreign statusLine)",
            (Self::ReplaceForeign, true) => "would install (replaces foreign statusLine)",
            (Self::Skip, _) => "skipped (foreign statusLine; --force replaces it)",
        }
    }
}

impl From<claude_settings::Step> for Step {
    fn from(step: claude_settings::Step) -> Self {
        match step {
            claude_settings::Step::Install => Self::Install,
            claude_settings::Step::Replace => Self::Replace,
            claude_settings::Step::Set => Self::Set,
        }
    }
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
    let mut settings = claude_settings::read(&claude_settings::path()).hinted()?;
    let before = settings.value.clone();
    let skill_step = if skill::state(&home, &SKILL, Target::Claude) == SkillState::Current {
        Step::Set
    } else {
        Step::Install
    };
    let mut steps = vec![("skill".to_owned(), skill_step)];
    steps.extend(wire(&mut settings, a.force));
    if !a.dry_run {
        if skill_step.writes() {
            skill::install(&home, &SKILL, Target::Claude).hinted()?;
        }
        if settings.value != before {
            claude_settings::write(&settings).hinted()?;
        }
    }
    let report = Report {
        dry_run: a.dry_run,
        settings: settings.path.display().to_string(),
        items: steps
            .into_iter()
            .map(|(name, step)| Item {
                name,
                status: step.text(a.dry_run),
            })
            .collect(),
    };
    print_report(&report, out)?;
    Ok(Outcome::Found)
}

/// Sets census's statusLine and hook for each event, returning what each item did.
fn wire(settings: &mut Settings, force: bool) -> Vec<(String, Step)> {
    let mut steps = vec![(
        "statusLine".to_owned(),
        wire_status_line(&mut settings.value, STATUS_LINE, force),
    )];
    for event in EVENTS {
        let owns = |command: &str| claude_settings::runs(command, TOOL, "hook");
        let step = claude_settings::set_hook(&mut settings.value, event, None, HOOK, owns);
        steps.push((format!("hook {event}"), step.into()));
    }
    steps
}

fn wire_status_line(settings: &mut Value, line: &str, force: bool) -> Step {
    let step = match settings.get("statusLine") {
        None => Step::Install,
        Some(value) if command_of(value) == Some(line) => Step::Set,
        Some(value)
            if command_of(value).is_some_and(|c| claude_settings::runs(c, TOOL, "statusline")) =>
        {
            Step::Replace
        }
        Some(_) if force => Step::ReplaceForeign,
        Some(_) => Step::Skip,
    };
    if step.writes() {
        settings["statusLine"] = json!({"type": "command", "command": line});
    }
    step
}

fn command_of(entry: &Value) -> Option<&str> {
    entry.get("command")?.as_str()
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

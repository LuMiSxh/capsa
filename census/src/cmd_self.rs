// SPDX-License-Identifier: MPL-2.0
//! `census self`: the skill, settings.json, the config, the health check, and the update and
//! removal of the binary.

use crate::out::Out;
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::Hinted;
use capsa_core::skill::{self, Skill, Target};
#[cfg(feature = "cmd-update")]
use std::path::Path;
#[cfg(feature = "cmd-update")]
use std::process::{Command, Stdio};

/// The skill shipped with this binary, so an installed copy matches its version.
pub(crate) const SKILL: Skill = Skill {
    tool: "census",
    text: include_str!("../skill/SKILL.md"),
};

/// This binary as the self-update and uninstall code sees it.
#[cfg(feature = "cmd-update")]
const TOOL: capsa_core::update::Tool = capsa_core::update::Tool {
    name: "census",
    version: env!("CARGO_PKG_VERSION"),
};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Replace this binary with the latest release, then reinstall the skill.
    Update(UpdateArgs),
    /// Print the skill for the main agent; --install writes it to ~/.claude/skills/census.
    Skill(SkillArgs),
    /// Installs the skill, then adds census's hooks and statusLine to settings.json.
    Setup(crate::setup::Args),
    /// Remove the installed skill, then this binary.
    Uninstall(UninstallArgs),
    /// Shows or sets the limit mode (plan or budget), the monthly budget and the daily limit.
    Config(crate::cmd_config::Args),
    /// Checks the binary, config, skill, hooks, state files and hook log; exits 1 on a failure.
    Doctor,
}

#[cfg(feature = "cmd-update")]
use capsa_core::update::{UninstallArgs, UpdateArgs};

#[cfg(not(feature = "cmd-update"))]
#[derive(clap::Args)]
pub struct UpdateArgs {}
#[cfg(not(feature = "cmd-update"))]
#[derive(clap::Args)]
pub struct UninstallArgs {
    /// Print what would be removed and remove nothing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(clap::Args)]
struct SkillArgs {
    /// Write SKILL.md to ~/.claude/skills/census instead of printing it.
    #[arg(long)]
    install: bool,
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    match a.action {
        Action::Skill(a) => skill(&a),
        Action::Setup(a) => crate::setup::run(&a, out),
        Action::Update(a) => update(a, out.format),
        Action::Uninstall(a) => uninstall(&a, out.format),
        Action::Config(a) => crate::cmd_config::run(a, out),
        Action::Doctor => crate::cmd_doctor::run(out),
    }
}

fn skill(a: &SkillArgs) -> Result<Outcome> {
    if !a.install {
        print!("{}", SKILL.text);
        return Ok(Outcome::Found);
    }
    let home = std::env::home_dir().context("no home directory")?;
    for file in skill::install(&home, &SKILL, Target::Claude)? {
        println!("{}", file.display());
    }
    Ok(Outcome::Found)
}

#[cfg(not(feature = "cmd-update"))]
fn update(_: UpdateArgs, _: Format) -> Result<Outcome> {
    Err(Hinted::new(
        "census was built without `cmd-update`",
        "cargo install --path census --features cmd-update, or rerun the installer",
    )
    .into())
}

#[cfg(feature = "cmd-update")]
fn update(a: UpdateArgs, format: Format) -> Result<Outcome> {
    capsa_core::update::run(&TOOL, a, format, |report| {
        reinstall_skill(&report.exe, format)
    })
}

/// Runs the new binary's `self skill --install`, since the binary on disk carries the skill of its
/// version. Under `--json` its path line is dropped, so stdout holds only the report.
#[cfg(feature = "cmd-update")]
fn reinstall_skill(exe: &Path, format: Format) -> Result<()> {
    let mut command = Command::new(exe);
    command.args(["self", "skill", "--install"]);
    if format == Format::Json {
        command.stdout(Stdio::null());
    }
    if command.status().is_ok_and(|s| s.success()) {
        return Ok(());
    }
    Err(Hinted::new(
        "cannot reinstall the skill",
        "run census self skill --install",
    )
    .into())
}

#[cfg(not(feature = "cmd-update"))]
fn uninstall(_: &UninstallArgs, _: Format) -> Result<Outcome> {
    Err(Hinted::new(
        "census was built without `cmd-update`",
        "remove the binary by hand, or rebuild with --features cmd-update",
    )
    .into())
}

/// Removes the binary first and the skill after it, so a refused binary leaves the skill in place
/// and a failed removal can run again. The check comes before both. Under `--json` stdout holds
/// only the removal.
#[cfg(feature = "cmd-update")]
fn uninstall(a: &UninstallArgs, format: Format) -> Result<Outcome> {
    let home = std::env::home_dir().context("no home directory")?;
    let outcome = capsa_core::update::uninstall_run(&TOOL, a.dry_run, format, |dry| {
        let files = skill::uninstall(&home, TOOL.name, dry)?;
        if format != Format::Json {
            let verb = if dry { "would remove" } else { "removed" };
            for file in &files {
                println!("{verb} {}", file.display());
            }
        }
        Ok(())
    })?;
    if format != Format::Json && !a.dry_run {
        println!(
            "hooks and statusLine in ~/.claude/settings.json are left; remove the census entries by hand"
        );
    }
    Ok(outcome)
}

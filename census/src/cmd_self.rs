// SPDX-License-Identifier: MPL-2.0
//! `census self`: prints and installs the agent skill, sets up settings.json, updates the binary,
//! removes it.

use crate::out::Out;
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::hint::Hinted;
use std::path::{Path, PathBuf};

/// The skill shipped with this binary, so an installed copy matches its version.
pub(crate) const SKILL: &str = include_str!("../skill/SKILL.md");

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
        Action::Update(a) => update(a),
        Action::Uninstall(a) => uninstall(&a),
    }
}

pub(crate) fn skill_file(home: &Path) -> PathBuf {
    home.join(".claude/skills/census/SKILL.md")
}

fn skill(a: &SkillArgs) -> Result<Outcome> {
    if !a.install {
        print!("{SKILL}");
        return Ok(Outcome::Found);
    }
    let home = std::env::home_dir().context("no home directory")?;
    println!("{}", install_skill(&home)?.display());
    Ok(Outcome::Found)
}

/// Writes the skill under `home`, creating its directory, and returns the file.
pub(crate) fn install_skill(home: &Path) -> Result<PathBuf> {
    let file = skill_file(home);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    std::fs::write(&file, SKILL).with_context(|| format!("cannot write {}", file.display()))?;
    Ok(file)
}

#[cfg(not(feature = "cmd-update"))]
fn update(_: UpdateArgs) -> Result<Outcome> {
    Err(Hinted::new(
        "census was built without `cmd-update`",
        "cargo install --path census --features cmd-update, or rerun the installer",
    )
    .into())
}

#[cfg(feature = "cmd-update")]
fn update(a: UpdateArgs) -> Result<Outcome> {
    use capsa_core::hint::ResultExt;

    let report = capsa_core::update::update(&TOOL, a.mode()).hinted()?;
    println!("{report}");
    if a.check {
        return Ok(report.outcome(true));
    }
    // The binary on disk carries the skill of its version; reinstall it.
    let ok = std::process::Command::new(&report.exe)
        .args(["self", "skill", "--install"])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err(Hinted::new(
            "cannot reinstall the skill",
            "run census self skill --install",
        )
        .into());
    }
    Ok(Outcome::Found)
}

#[cfg(not(feature = "cmd-update"))]
fn uninstall(_: &UninstallArgs) -> Result<Outcome> {
    Err(Hinted::new(
        "census was built without `cmd-update`",
        "remove the binary by hand, or rebuild with --features cmd-update",
    )
    .into())
}

/// Removes the skill first, so none outlives the binary it describes. The binary is checked
/// beforehand: a refusal there leaves the skill in place.
#[cfg(feature = "cmd-update")]
fn uninstall(a: &UninstallArgs) -> Result<Outcome> {
    use capsa_core::hint::ResultExt;
    use capsa_core::update::uninstall;

    let home = std::env::home_dir().context("no home directory")?;
    uninstall(&TOOL, true).hinted()?;
    let file = skill_file(&home);
    if file.is_file() {
        let verb = if a.dry_run { "would remove" } else { "removed" };
        if !a.dry_run {
            std::fs::remove_file(&file)
                .with_context(|| format!("cannot remove {}", file.display()))?;
            // Only an empty directory goes; anything else in it is the user's.
            if let Some(dir) = file.parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
        println!("{verb} {}", file.display());
    }
    println!("{}", uninstall(&TOOL, a.dry_run).hinted()?);
    if !a.dry_run {
        println!(
            "hooks and statusLine in ~/.claude/settings.json are left; remove the census entries by hand"
        );
    }
    Ok(Outcome::Found)
}

// SPDX-License-Identifier: MPL-2.0
//! `acus self update`: replaces this binary with the latest release, then reinstalls the skill and
//! adds the hooks that release registers.

use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;

#[cfg(feature = "cmd-update")]
use capsa_core::hint::hinted;
#[cfg(feature = "cmd-update")]
pub use capsa_core::update::UpdateArgs as Args;

#[cfg(not(feature = "cmd-update"))]
#[derive(clap::Args)]
pub struct Args {}

#[cfg(not(feature = "cmd-update"))]
pub fn run(_: Args, _format: Format) -> Result<Outcome> {
    Err(capsa_core::hint::update_unavailable(
        "acus",
        "crates/acus-cli",
    ))
}

#[cfg(feature = "cmd-update")]
pub fn run(a: Args, format: Format) -> Result<Outcome> {
    capsa_core::update::run(&crate::TOOL, a, format, |report| {
        let skill = skill_target().and_then(|target| {
            capsa_core::update::reinstall_skill(&report.exe, "acus", Some(target), format)
        });
        skill.and(add_missing_hooks(report, format))
    })
}

/// Adds the hooks that the new release registers and the old settings lack. The new binary runs
/// this, because only it knows its hooks. It prints nothing when no hook is missing.
///
/// A failure fails the update, because the hooks are part of the release. The child's own `error:`
/// and `hint:` lines reach stderr; under `--json` its stdout is dropped.
#[cfg(feature = "cmd-update")]
fn add_missing_hooks(report: &capsa_core::update::Report, format: Format) -> Result<()> {
    use std::process::{Command, Stdio};

    let mut child = Command::new(&report.exe);
    child.args(["self", "setup", "--add-missing"]);
    if format == Format::Json {
        child.stdout(Stdio::null());
    }
    if child.status().is_ok_and(|s| s.success()) {
        Ok(())
    } else {
        Err(hinted(
            "cannot add the hooks of this release",
            "run acus self setup",
        ))
    }
}

/// Returns the agent target that already holds the skill, so the reinstall keeps it there.
#[cfg(feature = "cmd-update")]
fn skill_target() -> Result<&'static str> {
    use anyhow::Context;
    use capsa_core::skill::{Target, installed_target};

    let home = std::env::home_dir().context("no home directory")?;
    Ok(
        match installed_target(&home, crate::cmd_skill::SKILL_DEF.tool) {
            Some(Target::Codex) => "codex",
            Some(Target::All) => "all",
            Some(Target::Claude) | None => "claude",
        },
    )
}

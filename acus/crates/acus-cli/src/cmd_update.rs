// SPDX-License-Identifier: MPL-2.0
//! `acus self update`: replaces this binary with the latest release, then reinstalls the skill and
//! adds the hooks that release registers.

use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;

#[cfg(feature = "cmd-update")]
pub use capsa_core::update::UpdateArgs as Args;

#[cfg(not(feature = "cmd-update"))]
#[derive(clap::Args)]
pub struct Args {}

#[cfg(not(feature = "cmd-update"))]
pub fn run(_: Args, _format: Format) -> Result<Outcome> {
    Err(capsa_core::hint::Hinted::new(
        "acus was built without `update`",
        "cargo install --path crates/acus-cli --features cmd-update, or rerun the installer",
    )
    .into())
}

#[cfg(feature = "cmd-update")]
pub fn run(a: Args, format: Format) -> Result<Outcome> {
    capsa_core::update::run(&crate::TOOL, a, format, |report| {
        let skill = reinstall_skill(report, format);
        add_missing_hooks(report, format);
        skill
    })
}

/// Adds the hooks that the new release registers and the old settings lack. The new binary runs
/// this, because only it knows its hooks. It prints nothing when no hook is missing.
///
/// A failure leaves the update successful: the binary is already replaced, and the child's own
/// `error:` and `hint:` lines reach stderr.
#[cfg(feature = "cmd-update")]
fn add_missing_hooks(report: &capsa_core::update::Report, format: Format) {
    use std::process::{Command, Stdio};

    let mut child = Command::new(&report.exe);
    child.args(["self", "setup", "--add-missing"]);
    if format == Format::Json {
        child.stdout(Stdio::null());
    }
    let _status = child.status();
}

/// Reinstalls the skill in the agent directories that already hold it. The binary on disk carries
/// the skill of its version, so this runs the new binary. Under `--json` its stdout is dropped,
/// so the update still prints one JSON value.
#[cfg(feature = "cmd-update")]
fn reinstall_skill(report: &capsa_core::update::Report, format: Format) -> Result<()> {
    use anyhow::Context;
    use capsa_core::hint::Hinted;
    use capsa_core::skill::{Target, installed_target};
    use std::process::{Command, Stdio};

    let home = std::env::home_dir().context("no home directory")?;
    let target = match installed_target(&home, crate::cmd_skill::SKILL_DEF.tool) {
        Some(Target::Codex) => "codex",
        Some(Target::All) => "all",
        Some(Target::Claude) | None => "claude",
    };
    let mut child = Command::new(&report.exe);
    child.args(["self", "skill", "--install", "--target", target]);
    if format == Format::Json {
        child.stdout(Stdio::null());
    }
    if child.status().is_ok_and(|s| s.success()) {
        Ok(())
    } else {
        Err(Hinted::new(
            "cannot reinstall the skill",
            "run acus self skill --install",
        )
        .into())
    }
}

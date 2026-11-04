// SPDX-License-Identifier: MPL-2.0
//! `acus self uninstall`: removes the skill acus installed, then the binary.

use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;

#[derive(clap::Args)]
pub struct Args {
    /// Print what would be removed and remove nothing.
    #[arg(long)]
    dry_run: bool,
}

#[cfg(not(feature = "cmd-update"))]
pub fn run(_: &Args, _format: Format) -> Result<Outcome> {
    Err(capsa_core::hint::Hinted::new(
        "acus was built without `uninstall`",
        "remove the binary by hand, or rebuild with --features cmd-update",
    )
    .into())
}

/// Removes the skill files first, so no skill outlives the binary it describes. The binary is
/// checked beforehand: a refusal there leaves the skill in place. The per-file lines and the
/// hooks note are for people and agents, so they are left out under `--json`.
#[cfg(feature = "cmd-update")]
pub fn run(a: &Args, format: Format) -> Result<Outcome> {
    use anyhow::Context;
    use capsa_core::hint::ResultExt;
    use capsa_core::update::uninstall_run;

    let home = std::env::home_dir().context("no home directory")?;
    let outcome = uninstall_run(&crate::TOOL, a.dry_run, format, |dry_run| {
        let files = capsa_core::skill::uninstall(&home, crate::cmd_skill::SKILL_DEF.tool, dry_run)
            .hinted()?;
        if format != Format::Json {
            for file in files {
                outln!("{} {}", verb(dry_run), file.display());
            }
        }
        Ok(())
    })?;
    if !a.dry_run && format != Format::Json {
        outln!(
            "hooks in {} are left; remove the acus entries by hand",
            capsa_core::claude_settings::path().display()
        );
    }
    Ok(outcome)
}

#[cfg(feature = "cmd-update")]
fn verb(dry_run: bool) -> &'static str {
    if dry_run { "would remove" } else { "removed" }
}

// SPDX-License-Identifier: MPL-2.0
//! `acus self`: the commands about acus itself, as opposed to the work it does on code.

use crate::config::Config;
use crate::{cmd_doctor, cmd_setup, cmd_skill, cmd_uninstall, cmd_update};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Replace this binary with the latest GitHub release and reinstall the skill.
    Update(cmd_update::Args),
    /// Install the skill and register the SessionStart and PreToolUse hooks in Claude Code's settings.
    Setup(cmd_setup::Args),
    /// Check the install, config, skill, hooks and guard files; exit 1 when a check fails.
    Doctor,
    /// Print the agent skill (for a SessionStart hook) or install it with `--install`.
    Skill(cmd_skill::Args),
    /// Remove this binary and the installed agent skill; hooks in Claude Code's settings stay.
    Uninstall(cmd_uninstall::Args),
}

impl Args {
    /// Whether this is `self doctor`, which must run even when the config file does not load.
    pub fn is_doctor(&self) -> bool {
        matches!(self.action, Action::Doctor)
    }

    /// The name `[commands] disabled` and `ACUS_DISABLE` use for this action.
    pub fn name(&self) -> &'static str {
        match self.action {
            Action::Update(_) => "update",
            Action::Setup(_) => "setup",
            Action::Doctor => "doctor",
            Action::Skill(_) => "skill",
            Action::Uninstall(_) => "uninstall",
        }
    }
}

pub fn run(a: Args, cfg: &Config, format: Format) -> Result<Outcome> {
    match a.action {
        Action::Update(a) => cmd_update::run(a, format),
        Action::Setup(a) => cmd_setup::run(&a, format),
        Action::Doctor => cmd_doctor::run(format),
        Action::Skill(a) => cmd_skill::run(a, cfg),
        Action::Uninstall(a) => cmd_uninstall::run(&a, format),
    }
}

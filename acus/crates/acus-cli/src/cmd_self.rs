// SPDX-License-Identifier: MPL-2.0
//! `acus self`: the commands about acus itself, as opposed to the work it does on code.

use crate::config::Config;
use crate::{cmd_skill, cmd_uninstall, cmd_update};
use anyhow::Result;
use capsa_core::exit::Outcome;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Replace this binary with the latest GitHub release and reinstall the skill.
    Update(cmd_update::Args),
    /// Print the agent skill (for a SessionStart hook) or install it with `--install`.
    Skill(cmd_skill::Args),
    /// Remove this binary and the installed agent skill; hooks in ~/.claude/settings.json stay.
    Uninstall(cmd_uninstall::Args),
}

impl Args {
    /// The name `[commands] disabled` and `ACUS_DISABLE` use for this action.
    pub fn name(&self) -> &'static str {
        match self.action {
            Action::Update(_) => "update",
            Action::Skill(_) => "skill",
            Action::Uninstall(_) => "uninstall",
        }
    }
}

pub fn run(a: Args, cfg: &Config) -> Result<Outcome> {
    match a.action {
        Action::Update(a) => cmd_update::run(a),
        Action::Skill(a) => cmd_skill::run(a, cfg),
        Action::Uninstall(a) => cmd_uninstall::run(&a),
    }
}

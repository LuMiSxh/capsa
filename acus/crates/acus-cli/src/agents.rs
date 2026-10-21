// SPDX-License-Identifier: MPL-2.0
//! The agents (Claude Code, Codex) that get the acus skill, and where each one reads it.

use std::path::{Path, PathBuf};

/// Skill directories under the home dir: Claude Code reads the first, Codex the second.
pub const CLAUDE_DIR: &str = ".claude/skills/acus";
pub const CODEX_DIR: &str = ".agents/skills/acus";

/// The agent whose skill directory `--install` writes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Agent {
    /// ~/.claude/skills/acus/ (default).
    Claude,
    /// ~/.agents/skills/acus/, which Codex reads.
    Codex,
    /// Both skill directories.
    All,
}

impl Agent {
    /// The `--agent` value for this agent.
    #[cfg(feature = "cmd-update")]
    pub fn flag(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::All => "all",
        }
    }

    /// Skill directories written for this agent, under `home`.
    pub fn dirs(self, home: &Path) -> Vec<PathBuf> {
        let claude = home.join(CLAUDE_DIR);
        let codex = home.join(CODEX_DIR);
        match self {
            Agent::Claude => vec![claude],
            Agent::Codex => vec![codex],
            Agent::All => vec![claude, codex],
        }
    }
}

/// The agent whose skill is already installed under `home`, so an update reinstalls it there;
/// claude when neither skill directory exists.
#[cfg(feature = "cmd-update")]
pub fn installed_agent(home: &Path) -> Agent {
    match (
        home.join(CLAUDE_DIR).is_dir(),
        home.join(CODEX_DIR).is_dir(),
    ) {
        (true, true) => Agent::All,
        (false, true) => Agent::Codex,
        _ => Agent::Claude,
    }
}

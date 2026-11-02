// SPDX-License-Identifier: MPL-2.0
//! Installs, checks and removes a tool's agent skill (`SKILL.md`) for Claude Code and Codex.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::hint::Hint;

#[cfg(test)]
mod tests;

/// The skill a tool ships: its name, which is also its directory name, and the file text.
#[derive(Debug, Clone, Copy)]
pub struct Skill {
    pub tool: &'static str,
    pub text: &'static str,
}

/// Where a skill is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Target {
    /// `~/.claude/skills/<tool>`, which Claude Code reads.
    Claude,
    /// `~/.agents/skills/<tool>`, which Codex reads.
    Codex,
    /// Both directories.
    All,
}

const SKILL_FILE: &str = "SKILL.md";

impl Target {
    /// The skill directories of `tool` under `home` that this target covers.
    pub fn dirs(self, home: &Path, tool: &str) -> Vec<PathBuf> {
        let claude = home.join(".claude").join("skills").join(tool);
        let codex = home.join(".agents").join("skills").join(tool);
        match self {
            Self::Claude => vec![claude],
            Self::Codex => vec![codex],
            Self::All => vec![claude, codex],
        }
    }
}

/// Whether the skill files of a target match the text the binary ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillState {
    /// No `SKILL.md` exists in any of the target's directories.
    Missing,
    /// At least one `SKILL.md` exists and differs from the shipped text, or cannot be read.
    Outdated,
    /// Every `SKILL.md` that exists matches the shipped text.
    Current,
}

/// Writes `skill` into the directories of `target` under `home` and returns the files written.
///
/// # Errors
///
/// Fails when a directory cannot be created or a file cannot be written. Directories written
/// before the failure are left in place.
pub fn install(home: &Path, skill: &Skill, target: Target) -> Result<Vec<PathBuf>, SkillError> {
    let mut written = Vec::new();
    for dir in target.dirs(home, skill.tool) {
        std::fs::create_dir_all(&dir).map_err(|source| SkillError::Create {
            path: dir.clone(),
            source,
        })?;
        let file = dir.join(SKILL_FILE);
        std::fs::write(&file, skill.text).map_err(|source| SkillError::Write {
            path: file.clone(),
            source,
        })?;
        written.push(file);
    }
    Ok(written)
}

/// Removes the `SKILL.md` of `tool` from every skill directory under `home`, and each directory
/// that is left empty. Returns the files removed or, with `dry_run`, the files that would be.
///
/// # Errors
///
/// Fails when a file exists but cannot be removed.
pub fn uninstall(home: &Path, tool: &str, dry_run: bool) -> Result<Vec<PathBuf>, SkillError> {
    let mut files = Vec::new();
    for dir in Target::All.dirs(home, tool) {
        let file = dir.join(SKILL_FILE);
        if !file.is_file() {
            continue;
        }
        if !dry_run {
            std::fs::remove_file(&file).map_err(|source| SkillError::Remove {
                path: file.clone(),
                source,
            })?;
            // A directory that still holds other files is the user's and stays.
            let _ = std::fs::remove_dir(&dir);
        }
        files.push(file);
    }
    Ok(files)
}

/// Compares the `SKILL.md` files of `target` with `skill.text`.
pub fn state(home: &Path, skill: &Skill, target: Target) -> SkillState {
    let mut present = false;
    for dir in target.dirs(home, skill.tool) {
        match std::fs::read_to_string(dir.join(SKILL_FILE)) {
            Ok(text) if text == skill.text => present = true,
            Ok(_) => return SkillState::Outdated,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return SkillState::Outdated,
        }
    }
    if present {
        SkillState::Current
    } else {
        SkillState::Missing
    }
}

/// The target whose skill directories of `tool` exist under `home`, or `None` when neither does.
pub fn installed_target(home: &Path, tool: &str) -> Option<Target> {
    let [claude, codex] = Target::All.dirs(home, tool).try_into().ok()?;
    match (claude.is_dir(), codex.is_dir()) {
        (true, true) => Some(Target::All),
        (true, false) => Some(Target::Claude),
        (false, true) => Some(Target::Codex),
        (false, false) => None,
    }
}

/// Why a skill could not be installed or removed.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("cannot create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot remove {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Hint for SkillError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(match self {
            Self::Create { .. } | Self::Write { .. } | Self::Remove { .. } => {
                "check the permissions of the skill directory, or remove it by hand"
            }
        }))
    }
}

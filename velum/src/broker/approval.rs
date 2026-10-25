// SPDX-License-Identifier: MPL-2.0
//! Owns the approval questions and what is remembered: per repository and subject in
//! `~/Library/Application Support/velum/approvals.toml`.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Whether an approval may be stored for later requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remember {
    /// Every request is asked again.
    Never,
    /// The approval may be stored for the repository and the request's subject.
    Repo,
}

/// The kind of request an approval covers and whether it may be remembered.
#[derive(Debug, Clone, Copy)]
pub struct Gate {
    pub class: &'static str,
    pub remember: Remember,
}

/// What the person chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Deny,
    Once,
    Repo,
}

impl Choice {
    /// Returns whether a gate with `remember` offers this choice.
    pub fn offered(self, remember: Remember) -> bool {
        match self {
            Self::Deny | Self::Once => true,
            Self::Repo => remember == Remember::Repo,
        }
    }
}

/// The question for the person: what the agent asks, and where.
#[derive(Debug)]
pub struct Prompt {
    pub tool: &'static str,
    pub repo: PathBuf,
    /// Labelled values. The values are agent-supplied or repository-supplied data.
    pub details: Vec<(&'static str, String)>,
    pub remember: Remember,
}

/// Asks the person. The sandbox must have no way to answer for them.
pub trait Approver: Send + Sync {
    /// Returns the choice; anything that is not a clear answer is `Deny`.
    fn ask(&self, prompt: &Prompt) -> Choice;
}

/// One stored approval. `subject` is what else the approval is bound to, such as the push URL.
#[derive(Debug, Serialize, Deserialize)]
struct Approval {
    class: String,
    repo: String,
    #[serde(default)]
    subject: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    approval: Vec<Approval>,
}

/// The stored approvals. The file is read anew on every call.
pub struct Memory {
    path: PathBuf,
}

impl Memory {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// A missing or unreadable file approves nothing.
    fn load(&self) -> File {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Returns whether an earlier "for this repo" approval covers `class` in `repo` for
    /// `subject`. A gate that is never remembered is never covered.
    pub fn covers(&self, gate: Gate, repo: &Path, subject: &str) -> bool {
        let repo = repo.to_string_lossy();
        gate.remember == Remember::Repo
            && self.load().approval.iter().any(|stored| {
                stored.class == gate.class && stored.repo == repo && stored.subject == subject
            })
    }

    /// Stores `choice` when the gate offers it. `Once` and `Deny` store nothing. The file is
    /// replaced by writing a new one beside it and renaming.
    pub fn remember(
        &self,
        gate: Gate,
        repo: &Path,
        subject: &str,
        choice: Choice,
    ) -> std::io::Result<()> {
        if choice != Choice::Repo || !choice.offered(gate.remember) {
            return Ok(());
        }
        if self.covers(gate, repo, subject) {
            return Ok(());
        }
        let mut all = self.load();
        all.approval.push(Approval {
            class: gate.class.to_string(),
            repo: repo.to_string_lossy().into_owned(),
            subject: subject.to_string(),
        });
        let text = toml::to_string(&all).map_err(std::io::Error::other)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = self
            .path
            .with_extension(format!("toml.{}", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        std::fs::rename(&temp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    const REPO_GATE: Gate = Gate {
        class: "git_push",
        remember: Remember::Repo,
    };
    const NEVER_GATE: Gate = Gate {
        class: "run_outside",
        remember: Remember::Never,
    };
    const URL: &str = "git@github.com:o/r.git";

    fn memory(scratch: &Scratch) -> Memory {
        Memory::new(scratch.path().join("approvals.toml"))
    }

    #[test]
    fn once_remembers_nothing() {
        let scratch = Scratch::new("approval-once");
        let memory = memory(&scratch);
        let repo = Path::new("/work/a");
        memory.remember(REPO_GATE, repo, URL, Choice::Once).unwrap();
        assert!(!memory.covers(REPO_GATE, repo, URL));
        assert!(!scratch.path().join("approvals.toml").exists());
    }

    #[test]
    fn a_repo_approval_is_stored_and_found_by_a_new_memory() {
        let scratch = Scratch::new("approval-repo");
        let repo = Path::new("/work/a");
        memory(&scratch)
            .remember(REPO_GATE, repo, URL, Choice::Repo)
            .unwrap();
        let fresh = memory(&scratch);
        assert!(fresh.covers(REPO_GATE, repo, URL));
        assert!(!fresh.covers(REPO_GATE, Path::new("/work/b"), URL));
        let other_class = Gate {
            class: "gh_read",
            ..REPO_GATE
        };
        assert!(!fresh.covers(other_class, repo, URL));
        let text = std::fs::read_to_string(scratch.path().join("approvals.toml")).unwrap();
        assert!(
            text.contains("git_push") && text.contains("/work/a"),
            "{text}"
        );
    }

    #[test]
    fn a_different_subject_asks_again() {
        let scratch = Scratch::new("approval-subject");
        let memory = memory(&scratch);
        let repo = Path::new("/work/a");
        memory.remember(REPO_GATE, repo, URL, Choice::Repo).unwrap();
        assert!(!memory.covers(REPO_GATE, repo, "git@github.com:evil/r.git"));
        assert!(!memory.covers(REPO_GATE, repo, ""));
        memory
            .remember(REPO_GATE, repo, "git@github.com:other/r.git", Choice::Repo)
            .unwrap();
        assert!(memory.covers(REPO_GATE, repo, URL));
    }

    #[test]
    fn a_gate_that_is_never_remembered_ignores_any_stored_choice() {
        let scratch = Scratch::new("approval-never");
        let memory = memory(&scratch);
        let repo = Path::new("/work/a");
        memory.remember(NEVER_GATE, repo, "", Choice::Repo).unwrap();
        assert!(!scratch.path().join("approvals.toml").exists());
        let planted = Gate {
            remember: Remember::Repo,
            ..NEVER_GATE
        };
        memory.remember(planted, repo, "", Choice::Repo).unwrap();
        assert!(!memory.covers(NEVER_GATE, repo, ""));
    }

    #[test]
    fn an_unreadable_approvals_file_approves_nothing() {
        let scratch = Scratch::new("approval-garbage");
        std::fs::write(scratch.path().join("approvals.toml"), "not [ toml").unwrap();
        assert!(!memory(&scratch).covers(REPO_GATE, Path::new("/work/a"), URL));
    }
}

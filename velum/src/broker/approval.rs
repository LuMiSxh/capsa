// SPDX-License-Identifier: MPL-2.0
//! Owns the approval questions and what is remembered: per session in memory, per repository
//! in `~/Library/Application Support/velum/approvals.toml`.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// How far an approval may reach beyond the one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remember {
    /// Every request is asked again.
    Never,
    /// The approval holds until the run ends.
    Session,
    /// The approval may also be stored for the repository.
    Repo,
}

/// The kind of request an approval covers and how far it may be remembered.
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
    Session,
    Repo,
}

impl Choice {
    /// Returns whether a gate with `remember` offers this choice.
    pub fn offered(self, remember: Remember) -> bool {
        match self {
            Self::Deny | Self::Once => true,
            Self::Session => remember == Remember::Session,
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

#[derive(Debug, Default, Serialize, Deserialize)]
struct Repos {
    #[serde(default)]
    repos: Vec<String>,
}

/// The stored approvals: one table per class with the repositories it covers.
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Reads the file anew on every call. A missing or unreadable file approves nothing.
    fn load(&self) -> BTreeMap<String, Repos> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn allows(&self, class: &str, repo: &Path) -> bool {
        let repo = repo.to_string_lossy();
        self.load()
            .get(class)
            .is_some_and(|entry| entry.repos.iter().any(|stored| *stored == repo))
    }

    /// Adds `repo` to `class` by writing a new file beside the old one and renaming it.
    pub fn add(&self, class: &str, repo: &Path) -> std::io::Result<()> {
        let mut all = self.load();
        let entry = all.entry(class.to_string()).or_default();
        let repo = repo.to_string_lossy().into_owned();
        if entry.repos.contains(&repo) {
            return Ok(());
        }
        entry.repos.push(repo);
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

/// Remembers approvals for the session and, through the store, for repositories.
pub struct Memory {
    session: Mutex<HashSet<(&'static str, PathBuf)>>,
    store: Store,
}

impl Memory {
    pub fn new(store: Store) -> Self {
        Self {
            session: Mutex::new(HashSet::new()),
            store,
        }
    }

    /// Returns whether an earlier approval covers `gate` in `repo`. A gate that is never
    /// remembered is never covered.
    pub fn covers(&self, gate: Gate, repo: &Path) -> bool {
        match gate.remember {
            Remember::Never => false,
            Remember::Session => self.in_session(gate.class, repo),
            Remember::Repo => {
                self.in_session(gate.class, repo) || self.store.allows(gate.class, repo)
            }
        }
    }

    fn in_session(&self, class: &'static str, repo: &Path) -> bool {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&(class, repo.to_path_buf()))
    }

    /// Records `choice` for later requests of the same class in `repo`. `Once` and `Deny`
    /// record nothing, and neither does a choice the gate does not offer.
    pub fn remember(&self, gate: Gate, repo: &Path, choice: Choice) -> std::io::Result<()> {
        if !choice.offered(gate.remember) {
            return Ok(());
        }
        match choice {
            Choice::Session => {
                self.session
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert((gate.class, repo.to_path_buf()));
                Ok(())
            }
            Choice::Repo => self.store.add(gate.class, repo),
            Choice::Deny | Choice::Once => Ok(()),
        }
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
    const SESSION_GATE: Gate = Gate {
        class: "open_url",
        remember: Remember::Session,
    };
    const NEVER_GATE: Gate = Gate {
        class: "run_outside",
        remember: Remember::Never,
    };

    fn memory(scratch: &Scratch) -> Memory {
        Memory::new(Store::new(scratch.path().join("approvals.toml")))
    }

    #[test]
    fn once_remembers_nothing() {
        let scratch = Scratch::new("approval-once");
        let memory = memory(&scratch);
        let repo = Path::new("/work/a");
        memory.remember(REPO_GATE, repo, Choice::Once).unwrap();
        assert!(!memory.covers(REPO_GATE, repo));
    }

    #[test]
    fn a_session_approval_covers_its_class_and_repo_until_the_memory_ends() {
        let scratch = Scratch::new("approval-session");
        let memory = memory(&scratch);
        let repo = Path::new("/work/a");
        memory
            .remember(SESSION_GATE, repo, Choice::Session)
            .unwrap();
        assert!(memory.covers(SESSION_GATE, repo));
        assert!(!memory.covers(SESSION_GATE, Path::new("/work/b")));
        assert!(!scratch.path().join("approvals.toml").exists());
        assert!(!self::memory(&scratch).covers(SESSION_GATE, repo));
    }

    #[test]
    fn a_repo_approval_is_stored_and_found_by_a_new_memory() {
        let scratch = Scratch::new("approval-repo");
        let repo = Path::new("/work/a");
        memory(&scratch)
            .remember(REPO_GATE, repo, Choice::Repo)
            .unwrap();
        let fresh = memory(&scratch);
        assert!(fresh.covers(REPO_GATE, repo));
        assert!(!fresh.covers(REPO_GATE, Path::new("/work/b")));
        assert!(!fresh.covers(
            Gate {
                class: "gh_read",
                ..REPO_GATE
            },
            repo
        ));
        let text = std::fs::read_to_string(scratch.path().join("approvals.toml")).unwrap();
        assert!(
            text.contains("[git_push]") && text.contains("/work/a"),
            "{text}"
        );
    }

    #[test]
    fn a_gate_that_is_never_remembered_ignores_any_stored_choice() {
        let scratch = Scratch::new("approval-never");
        let memory = memory(&scratch);
        let repo = Path::new("/work/a");
        memory.remember(NEVER_GATE, repo, Choice::Repo).unwrap();
        memory.remember(NEVER_GATE, repo, Choice::Session).unwrap();
        assert!(!memory.covers(NEVER_GATE, repo));
        assert!(!scratch.path().join("approvals.toml").exists());
        let planted = Store::new(scratch.path().join("approvals.toml"));
        planted.add("run_outside", repo).unwrap();
        assert!(!self::memory(&scratch).covers(NEVER_GATE, repo));
    }

    #[test]
    fn a_repo_choice_is_ignored_by_a_session_gate() {
        let scratch = Scratch::new("approval-mismatch");
        let memory = memory(&scratch);
        let repo = Path::new("/work/a");
        memory.remember(SESSION_GATE, repo, Choice::Repo).unwrap();
        assert!(!memory.covers(SESSION_GATE, repo));
        assert!(!scratch.path().join("approvals.toml").exists());
    }

    #[test]
    fn an_unreadable_approvals_file_approves_nothing() {
        let scratch = Scratch::new("approval-garbage");
        std::fs::write(scratch.path().join("approvals.toml"), "not [ toml").unwrap();
        assert!(!memory(&scratch).covers(REPO_GATE, Path::new("/work/a")));
    }
}

// SPDX-License-Identifier: MPL-2.0
//! Registers a tool's hooks in a coding agent's settings, finds out whether they are current, and
//! leaves every other hook and setting as it was. A tool states its hooks as `HookSpec`s; where
//! the agent keeps them is the `Agent`'s business.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::claude_settings::{self, Settings, SettingsError, Step, remove_hook, set_hook};

mod check;
mod spec;
#[cfg(test)]
mod tests;

pub use spec::{HookSpec, HookState};

/// A coding agent that runs hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Agent {
    /// Claude Code, which keeps its hooks in `settings.json`.
    ClaudeCode,
}

impl Agent {
    /// Returns the file where the agent keeps its hooks.
    pub fn path(self) -> PathBuf {
        match self {
            Self::ClaudeCode => claude_settings::path(),
        }
    }
}

/// An agent's hooks document, opened for reading and editing.
///
/// Edits stay in memory until `write`. A write is refused when the file changed since it was
/// opened.
#[derive(Debug)]
pub struct HookFile {
    agent: Agent,
    settings: Settings,
}

impl HookFile {
    /// Opens the agent's own file. A missing file is an empty document.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or parsed, or holds hooks of the wrong shape.
    pub fn open(agent: Agent) -> Result<Self, SettingsError> {
        Self::open_at(agent, &agent.path())
    }

    /// Opens the document at `path`, which holds hooks in the form `agent` uses.
    ///
    /// # Errors
    ///
    /// Fails as `open` does.
    pub fn open_at(agent: Agent, path: &Path) -> Result<Self, SettingsError> {
        let settings = match agent {
            Agent::ClaudeCode => claude_settings::read(path)?,
        };
        Ok(Self { agent, settings })
    }

    /// Returns the file that `write` replaces: the target of a symlink, so the link stays.
    pub fn path(&self) -> &Path {
        &self.settings.path
    }

    /// Returns the parsed document.
    pub fn value(&self) -> &Value {
        &self.settings.value
    }

    /// Returns the parsed document for edits beyond hooks. `write` saves them with the hooks.
    pub fn value_mut(&mut self) -> &mut Value {
        &mut self.settings.value
    }

    /// Returns what the document holds for `spec`.
    pub fn state(&self, spec: &HookSpec) -> HookState {
        spec::state(self.value(), spec)
    }

    /// Makes the hook run `spec.command` under `spec.matcher`, replacing every entry that
    /// `spec.owns` claims.
    ///
    /// An identical entry gives `Step::Set`. A spec without a matcher keeps an entry with its
    /// command that the user scoped with one. Other hooks stay in place.
    pub fn set(&mut self, spec: &HookSpec) -> Step {
        if spec.matcher.is_none() && matches!(self.state(spec), HookState::CustomMatcher { .. }) {
            return Step::Set;
        }
        set_hook(
            self.value_mut(),
            spec.event,
            spec.matcher,
            spec.command,
            spec.owns,
        )
    }

    /// Adds the hook when the tool has no entry under its event, and replaces the entries that
    /// `spec.legacy` claims.
    ///
    /// An entry the tool already has stays, whatever its matcher or command, so an update never
    /// undoes a choice of the user.
    pub fn add(&mut self, spec: &HookSpec) -> Step {
        spec::add(self.value_mut(), spec)
    }

    /// Removes the entries that `spec.owns` claims, and the groups and event they leave empty.
    /// Returns whether an entry was removed.
    pub fn remove(&mut self, spec: &HookSpec) -> bool {
        remove_hook(self.value_mut(), spec.event, spec.owns)
    }

    /// Writes the document over its file, after copying the old file to `<file>.bak`.
    ///
    /// # Errors
    ///
    /// Fails with `Changed` when the file changed since it was opened, and with `Write` when a
    /// file cannot be written. A failed write leaves the file as it was.
    pub fn write(&self) -> Result<(), SettingsError> {
        match self.agent {
            Agent::ClaudeCode => claude_settings::write(&self.settings),
        }
    }
}

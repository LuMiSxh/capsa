// SPDX-License-Identifier: MPL-2.0
//! Owns the JavaScript package managers: which one to use and the global install of a
//! package. Nothing here prints; callers render the [`InstallReport`].

mod bin;
mod manifest;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::io;
use std::process::{Command, ExitStatus};

pub use bin::{global_bin_dir, global_bin_dir_in};
pub use manifest::{manifest_for, manifest_in, package_version};
use thiserror::Error;

use crate::hint::Hint;
use crate::which::{self, Env, Fs, RealFs};

/// A JavaScript package manager that installs global packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Pnpm,
    Bun,
    Npm,
}

impl Manager {
    /// Every manager, in the order [`detect`] tries them.
    pub const ALL: [Self; 3] = [Self::Pnpm, Self::Bun, Self::Npm];

    pub fn program(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
            Self::Npm => "npm",
        }
    }
}

/// The global install command for `package`, program first.
pub fn install_command(manager: Manager, package: &str) -> Vec<String> {
    let verb = match manager {
        Manager::Npm => "install",
        Manager::Pnpm | Manager::Bun => "add",
    };
    vec![
        manager.program().to_owned(),
        verb.to_owned(),
        "-g".to_owned(),
        package.to_owned(),
    ]
}

/// Returns `preferred` when it is on the PATH (or a user bin
/// directory), else the first of [`Manager::ALL`] that is.
pub fn detect(preferred: Option<Manager>) -> Option<Manager> {
    detect_in(&Env::from_process(), &RealFs, preferred)
}

/// Detects like [`detect`], over the given search environment and file system.
pub fn detect_in(env: &Env, fs: &dyn Fs, preferred: Option<Manager>) -> Option<Manager> {
    let on_path = |m: Manager| which::find_in(env, fs, m.program(), None).is_some();
    preferred
        .filter(|m| on_path(*m))
        .or_else(|| Manager::ALL.into_iter().find(|m| on_path(*m)))
}

/// Records what an install ran and how it ended.
#[derive(Debug)]
pub struct InstallReport {
    /// The command, program first.
    pub command: Vec<String>,
    /// The exit status; `None` for a dry run, which runs nothing.
    pub status: Option<ExitStatus>,
}

impl InstallReport {
    /// Returns true for a dry run and for a command that exited with 0.
    pub fn success(&self) -> bool {
        self.status.is_none_or(|status| status.success())
    }
}

/// A failure to find or start a package manager.
#[derive(Debug, Error)]
pub enum PkgError {
    /// None of pnpm, bun or npm is on the PATH.
    #[error("no package manager found: looked for pnpm, bun and npm")]
    NoManager,
    /// The manager's program is missing or could not be started.
    #[error("could not run {program}")]
    Spawn {
        program: &'static str,
        #[source]
        source: io::Error,
    },
}

impl Hint for PkgError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        Some(match self {
            Self::NoManager => Cow::Borrowed(
                "install Node.js (for example `brew install node`) or pnpm, then run this again",
            ),
            Self::Spawn { program, .. } => Cow::Owned(format!(
                "check that `{program}` runs in a terminal, or choose another package manager"
            )),
        })
    }
}

/// Installs `package` globally with `manager`. A dry run returns the command
/// and runs nothing.
pub fn install(manager: Manager, package: &str, dry_run: bool) -> Result<InstallReport, PkgError> {
    install_in(&Env::from_process(), &RealFs, manager, package, dry_run)
}

/// Installs like [`install`], over the given search environment and file system. The manager
/// is run from where it is found, with its own directory first on the child's
/// PATH, so that `node` beside it is found too.
pub fn install_in(
    env: &Env,
    fs: &dyn Fs,
    manager: Manager,
    package: &str,
    dry_run: bool,
) -> Result<InstallReport, PkgError> {
    let command = install_command(manager, package);
    if dry_run {
        return Ok(InstallReport {
            command,
            status: None,
        });
    }
    let name = manager.program();
    let found = which::find_in(env, fs, name, None).ok_or_else(|| PkgError::Spawn {
        program: name,
        source: io::Error::new(io::ErrorKind::NotFound, "not found on the PATH"),
    })?;
    let mut child = Command::new(&found);
    child.args(command.iter().skip(1));
    if let Ok(path) = std::env::join_paths(which::child_path(env, fs, &found)) {
        child.env("PATH", path);
    }
    let status = child.status().map_err(|source| PkgError::Spawn {
        program: name,
        source,
    })?;
    Ok(InstallReport {
        command,
        status: Some(status),
    })
}

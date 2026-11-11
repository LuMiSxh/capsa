// SPDX-License-Identifier: MPL-2.0
//! Owns updating and removing the running tool binary from the capsa GitHub releases.
//! [`update`] and [`uninstall`] return values; `run` and `uninstall_run` print them through
//! `render::emit` in the output format the tool was given.

mod archive;
#[cfg(feature = "clap")]
mod args;
mod binary;
mod error;
mod release;
mod steps;
#[cfg(test)]
mod tests;

use std::fmt;
use std::io;
use std::path::PathBuf;

use serde::Serialize;

use crate::exit::Outcome;
use crate::format::Format;
use crate::hint::ResultExt;
use crate::render::{Doc, Mark, Render, Status, emit};

#[cfg(feature = "clap")]
pub use args::{UninstallArgs, UpdateArgs};
pub use error::UpdateError;
pub use steps::{leftover_note, reinstall_skill};

/// The repository that holds the releases of every capsa tool.
pub const REPO: &str = "LuMiSxh/capsa";

/// The tool being updated: its binary name, which is also its release tag prefix and archive
/// name, and the version it runs.
#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub name: &'static str,
    pub version: &'static str,
}

/// What [`update`] may do once it knows the newest release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only report whether a newer release exists.
    Check,
    /// Replace the binary when the release is newer.
    Install,
    /// Replace the binary even when the release is not newer.
    Force,
}

/// Records what an update found or did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// [`Mode::Check`] found a newer release.
    Available,
    /// The installed version is the newest, or [`Mode::Install`] had nothing to do.
    Current,
    /// The binary was replaced.
    Updated,
}

/// Holds the result of [`update`]; its agent view is the `Display` line.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub tool: &'static str,
    pub current: String,
    pub latest: String,
    /// The resolved path of the running binary.
    pub exe: PathBuf,
    pub state: State,
}

impl Report {
    /// Returns the exit outcome: with `check`, exit 1 when no newer release exists; otherwise 0.
    pub fn outcome(&self, check: bool) -> Outcome {
        if check && self.state != State::Available {
            Outcome::Empty
        } else {
            Outcome::Found
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            tool,
            current,
            latest,
            exe,
            ..
        } = self;
        match self.state {
            State::Available => write!(
                f,
                "{tool} {latest} is available (installed {current}); run {tool} self update"
            ),
            State::Current => write!(f, "{tool} {current} is current ({})", exe.display()),
            State::Updated => {
                write!(f, "updated {} from {current} to {latest}", exe.display())
            }
        }
    }
}

impl Render for Report {
    fn doc(&self, doc: &mut Doc) {
        let (status, word) = match self.state {
            State::Available => (Status::Warn, "update available"),
            State::Current => (Status::Ok, "current"),
            State::Updated => (Status::Ok, "updated"),
        };
        doc.title(format!("{} self update", self.tool));
        doc.marks([Mark {
            status,
            text: word.to_owned(),
        }]);
        if self.state == State::Updated {
            doc.row("from").text(self.current.as_str());
            doc.row("to").text(self.latest.as_str());
        } else {
            doc.row("installed").text(self.current.as_str());
            doc.row("latest").text(self.latest.as_str());
        }
        doc.row("binary").text(self.exe.display().to_string());
        if self.state == State::Available {
            doc.hint(format!("run {} self update", self.tool));
        }
    }

    fn agent(&self, out: &mut dyn io::Write) -> io::Result<()> {
        writeln!(out, "{self}")
    }
}

/// Replaces the running binary with the newest release of `tool` according to `mode`.
///
/// A GitHub token from `GITHUB_TOKEN` or `gh auth token` is sent when there is one; it only lifts
/// the GitHub rate limit for anonymous requests.
///
/// # Errors
///
/// Fails on a network or release problem, when the checksum does not match (nothing is written
/// then), and when the binary cannot be replaced.
pub fn update(tool: &Tool, mode: Mode) -> Result<Report, UpdateError> {
    let exe = binary::resolve()?;
    binary::forget_old(&exe);
    let token = release::token();
    let newest = release::newest(tool, token.as_deref())?;
    let latest = newest.version(tool).to_owned();
    let newer = release::version(&latest) > release::version(tool.version);
    let install = match mode {
        Mode::Check => false,
        Mode::Install => newer,
        Mode::Force => true,
    };
    if install {
        let target = release::target()?;
        let (ext, bin) = archive::kind(tool);
        let name = release::archive_name(tool.name, target, ext);
        let data = newest.download(&name, token.as_deref())?;
        let sum = newest.download(&format!("{name}.sha256"), token.as_deref())?;
        archive::verify(&data, &String::from_utf8_lossy(&sum))?;
        binary::replace(&exe, tool.name, &archive::extract(&data, ext, &bin)?)?;
    }
    let state = match (mode, install, newer) {
        (_, true, _) => State::Updated,
        (Mode::Check, _, true) => State::Available,
        _ => State::Current,
    };
    Ok(Report {
        tool: tool.name,
        current: tool.version.to_owned(),
        latest,
        exe,
        state,
    })
}

/// Records the binary [`uninstall`] removed, or would remove.
#[derive(Debug, Clone, Serialize)]
pub struct Removal {
    pub path: PathBuf,
    pub removed: bool,
}

impl fmt::Display for Removal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = if self.removed {
            "removed"
        } else {
            "would remove"
        };
        write!(f, "{verb} {}", self.path.display())
    }
}

impl Render for Removal {
    fn doc(&self, doc: &mut Doc) {
        doc.title("self uninstall");
        let label = if self.removed {
            "removed"
        } else {
            "would remove"
        };
        doc.row(label).text(self.path.display().to_string());
    }

    fn agent(&self, out: &mut dyn io::Write) -> io::Result<()> {
        writeln!(out, "{self}")
    }
}

/// Removes the running binary of `tool`; with `dry_run` it only names it.
///
/// A symlink is resolved first, so the real file goes. A binary not named after the tool is
/// refused, so a renamed copy cannot be mistaken for the install.
///
/// # Errors
///
/// Fails when the binary cannot be located, is not named `tool`, or cannot be removed.
pub fn uninstall(tool: &Tool, dry_run: bool) -> Result<Removal, UpdateError> {
    let path = binary::resolve()?;
    binary::ensure_named(&path, tool.name)?;
    if !dry_run {
        binary::remove(&path)?;
    }
    Ok(Removal {
        path,
        removed: !dry_run,
    })
}

/// Runs `<tool> self update`: updates by `args`, prints the report in `format` and, unless it
/// only checked, calls `after` so the tool can reinstall its skill.
///
/// # Errors
///
/// Fails as [`update`] does, and when `after` fails. The report is printed before `after` runs.
#[cfg(feature = "clap")]
pub fn run(
    tool: &Tool,
    args: UpdateArgs,
    format: Format,
    after: impl FnOnce(&Report) -> anyhow::Result<()>,
) -> anyhow::Result<Outcome> {
    let report = update(tool, args.mode()).hinted()?;
    emit(&report, format)?;
    if !args.check {
        after(&report)?;
    }
    Ok(report.outcome(args.check))
}

/// Runs `<tool> self uninstall`: removes the binary, then runs `remove_extras` (the tool's own
/// files) and prints the removal.
///
/// The binary goes first because its removal is the step that can fail late (a locked exe on
/// Windows passes the check and then refuses the delete); a leftover skill file is harmless.
///
/// # Errors
///
/// Fails as [`uninstall`] does, and when `remove_extras` fails. A refused binary leaves the
/// extra files in place.
pub fn uninstall_run(
    tool: &Tool,
    dry_run: bool,
    format: Format,
    remove_extras: impl FnOnce(bool) -> anyhow::Result<()>,
) -> anyhow::Result<Outcome> {
    let removal = uninstall(tool, dry_run).hinted()?;
    remove_extras(dry_run)?;
    emit(&removal, format)
}

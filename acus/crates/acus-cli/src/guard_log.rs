// SPDX-License-Identifier: MPL-2.0
//! Owns where the guard log lives, and the one-time move from the config directory it used to
//! share.

use std::io;
use std::path::{Path, PathBuf};

const NAME: &str = "guard.jsonl";

/// Returns the guard log in the state directory, or beside `config` when no state directory is
/// known. Hooks call this on every decision, so it reads only the environment.
pub fn path(config: &Path) -> PathBuf {
    capsa_core::paths::log_file("acus", NAME).unwrap_or_else(|| config.with_file_name(NAME))
}

/// Returns the guard log files still beside `config`, the rotated copy first, while either
/// exists. `self setup` moves them and `self doctor` reports them, even after the state log has
/// been created: hooks create that log on the first decision after an upgrade.
pub fn leftovers(config: &Path) -> Vec<PathBuf> {
    let log = config.with_file_name(NAME);
    if path(config) == log {
        return Vec::new();
    }
    [rotated(&log), log]
        .into_iter()
        .filter(|file| file.exists())
        .collect()
}

/// One step of the move from beside `config` into the state directory.
pub enum Move {
    /// Renames a file into place, because the state log did not exist yet.
    Renamed { from: PathBuf, to: PathBuf },
    /// Writes the old lines in front of the state log, then removes the old files.
    Merged { from: Vec<PathBuf>, to: PathBuf },
}

/// Moves the guard log files from beside `config` into the state directory, and returns the steps.
/// Renames when no state log or rotated copy exists; otherwise merges the old lines in front of
/// the state log. `dry_run` returns the steps without touching files.
pub fn migrate(config: &Path, dry_run: bool) -> io::Result<Vec<Move>> {
    let from = leftovers(config);
    if from.is_empty() {
        return Ok(Vec::new());
    }
    let to = path(config);
    let moves = if to.exists() || rotated(&to).exists() {
        vec![Move::Merged { from, to }]
    } else {
        let log = config.with_file_name(NAME);
        [(log.clone(), to.clone()), (rotated(&log), rotated(&to))]
            .into_iter()
            .filter(|(from, _)| from.exists())
            .map(|(from, to)| Move::Renamed { from, to })
            .collect()
    };
    if !dry_run {
        for step in &moves {
            step.apply()?;
        }
    }
    Ok(moves)
}

impl Move {
    fn apply(&self) -> io::Result<()> {
        match self {
            Self::Renamed { from, to } => {
                create_parent(to)?;
                std::fs::rename(from, to)
            }
            Self::Merged { from, to } => merge(from, to),
        }
    }
}

/// Writes the lines of `from` in order, then the lines of `to` when it exists, into `to`
/// atomically, and removes `from`. The rotated copy of `to` is not read or changed.
fn merge(from: &[PathBuf], to: &Path) -> io::Result<()> {
    let mut sources = from.to_vec();
    if to.exists() {
        sources.push(to.to_path_buf());
    }
    let mut bytes = Vec::new();
    for file in &sources {
        let part = std::fs::read(file)?;
        bytes.extend_from_slice(&part);
        if !part.is_empty() && !part.ends_with(b"\n") {
            bytes.push(b'\n');
        }
    }
    create_parent(to)?;
    capsa_core::fs::write_atomic(to, &bytes)?;
    for file in from {
        std::fs::remove_file(file)?;
    }
    Ok(())
}

fn create_parent(file: &Path) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// Returns the rotated copy of a log, which `capsa_core::log` names by appending `.old`.
fn rotated(log: &Path) -> PathBuf {
    let mut name = log.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

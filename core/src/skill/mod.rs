// SPDX-License-Identifier: MPL-2.0
//! Installs, checks and removes a tool's agent skill (`SKILL.md`) for Claude Code and Codex.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use crate::hint::Hint;

#[cfg(test)]
mod tests;

/// Files a skill ships beside `SKILL.md`: a path relative to the skill directory and its text.
pub type ExtraFiles = &'static [(&'static str, &'static str)];

/// The skill a tool ships: its name, which is also its directory name, and its files.
#[derive(Debug, Clone, Copy)]
pub struct Skill {
    pub tool: &'static str,
    pub text: &'static str,
    pub extra: ExtraFiles,
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
/// Lists the files the last install wrote, one relative path per line.
const MANIFEST_FILE: &str = ".acus-files";

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
    /// At least one `SKILL.md` exists and differs from the shipped text, cannot be read, or sits
    /// beside a file an older version shipped.
    Outdated,
    /// Every `SKILL.md` that exists matches the shipped text.
    Current,
}

/// Writes `skill` into the directories of `target` under `home` and returns the files written.
/// Files an earlier install wrote that this one no longer ships are removed.
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
        let stale = stale_files(&dir, skill);
        write(&dir.join(SKILL_FILE), skill.text, &mut written)?;
        for (path, text) in skill.extra {
            let file = dir.join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent).map_err(|source| SkillError::Create {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            write(&file, text, &mut written)?;
        }
        remove_stale(&dir, &stale)?;
        write_manifest(&dir, skill)?;
    }
    Ok(written)
}

fn write(file: &Path, text: &str, written: &mut Vec<PathBuf>) -> Result<(), SkillError> {
    std::fs::write(file, text).map_err(|source| SkillError::Write {
        path: file.to_path_buf(),
        source,
    })?;
    written.push(file.to_path_buf());
    Ok(())
}

fn remove(file: &Path) -> Result<(), SkillError> {
    std::fs::remove_file(file).map_err(|source| SkillError::Remove {
        path: file.to_path_buf(),
        source,
    })
}

/// The files `skill` ships below its directory: `SKILL.md`, then the extra files.
fn shipped(skill: &Skill) -> Vec<&'static str> {
    std::iter::once(SKILL_FILE)
        .chain(skill.extra.iter().map(|&(path, _)| path))
        .collect()
}

fn write_manifest(dir: &Path, skill: &Skill) -> Result<(), SkillError> {
    let text: String = shipped(skill)
        .iter()
        .map(|file| format!("{file}\n"))
        .collect();
    let path = dir.join(MANIFEST_FILE);
    std::fs::write(&path, text).map_err(|source| SkillError::Write { path, source })
}

/// The files an earlier install left in `dir` that `skill` no longer ships. The manifest lists
/// them; without one, the `.md` files in the subdirectories `skill` ships in are the candidates.
fn stale_files(dir: &Path, skill: &Skill) -> Vec<String> {
    let current = shipped(skill);
    read_manifest(dir)
        .unwrap_or_else(|| legacy_files(dir, skill))
        .into_iter()
        .filter(|file| !current.contains(&file.as_str()))
        .collect()
}

/// The files the manifest in `dir` lists, or `None` when there is none. An entry that is absolute
/// or climbs out of `dir` is skipped, so a damaged manifest cannot delete files outside it.
fn read_manifest(dir: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(dir.join(MANIFEST_FILE)).ok()?;
    Some(
        text.lines()
            .map(str::trim)
            .filter(|line| is_relative_file(line))
            .map(str::to_owned)
            .collect(),
    )
}

fn is_relative_file(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// The `.md` files in the subdirectories `skill` ships in, as paths relative to `dir`. An install
/// from before the manifest recorded no files, so these are the only ones it can have left.
fn legacy_files(dir: &Path, skill: &Skill) -> Vec<String> {
    let mut subdirs: Vec<&str> = skill
        .extra
        .iter()
        .filter_map(|&(path, _)| path.rsplit_once('/').map(|(sub, _)| sub))
        .collect();
    subdirs.sort_unstable();
    subdirs.dedup();
    let mut files = Vec::new();
    for sub in subdirs {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.ends_with(".md") && entry.path().is_file() {
                files.push(format!("{sub}/{name}"));
            }
        }
    }
    files
}

/// Removes the files a previous install left behind that are still on disk.
fn remove_stale(dir: &Path, stale: &[String]) -> Result<(), SkillError> {
    for file in stale {
        let path = dir.join(file);
        if path.is_file() {
            remove(&path)?;
        }
    }
    remove_empty_dirs(dir, stale);
    Ok(())
}

/// Removes the files of `skill` and the files its manifest lists from every skill directory under
/// `home`, the manifest too, and each directory that is left empty. Returns the files removed or,
/// with `dry_run`, the files that would be.
///
/// # Errors
///
/// Fails when a file exists but cannot be removed.
pub fn uninstall(home: &Path, skill: &Skill, dry_run: bool) -> Result<Vec<PathBuf>, SkillError> {
    let mut files = Vec::new();
    for dir in Target::All.dirs(home, skill.tool) {
        let owned = owned_files(&dir, skill);
        for file in &owned {
            let path = dir.join(file);
            if path.is_file() {
                if !dry_run {
                    remove(&path)?;
                }
                files.push(path);
            }
        }
        if dry_run {
            continue;
        }
        let manifest = dir.join(MANIFEST_FILE);
        if manifest.is_file() {
            remove(&manifest)?;
        }
        remove_empty_dirs(&dir, &owned);
    }
    Ok(files)
}

/// The files `skill` owns in `dir`: what it ships, and what the manifest lists from older versions.
fn owned_files(dir: &Path, skill: &Skill) -> Vec<String> {
    let mut files: Vec<String> = shipped(skill).into_iter().map(str::to_owned).collect();
    for file in read_manifest(dir).unwrap_or_default() {
        if !files.contains(&file) {
            files.push(file);
        }
    }
    files
}

/// Removes the directories `files` sit in below `dir`, deepest first, where they are empty.
fn remove_empty_dirs(dir: &Path, files: &[String]) {
    let mut dirs: Vec<PathBuf> = files
        .iter()
        .filter_map(|path| dir.join(path).parent().map(Path::to_path_buf))
        .collect();
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    dirs.dedup();
    for owned in dirs {
        if owned != dir && owned.starts_with(dir) {
            // A directory that still holds other files is the user's and stays.
            let _ = std::fs::remove_dir(owned);
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Compares the files of `target` with the ones `skill` ships.
pub fn state(home: &Path, skill: &Skill, target: Target) -> SkillState {
    let mut present = false;
    for dir in target.dirs(home, skill.tool) {
        match std::fs::read_to_string(dir.join(SKILL_FILE)) {
            Ok(text) if text == skill.text => present = true,
            Ok(_) => return SkillState::Outdated,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return SkillState::Outdated,
        }
        for (path, text) in skill.extra {
            match std::fs::read_to_string(dir.join(path)) {
                Ok(found) if found == *text => {}
                _ => return SkillState::Outdated,
            }
        }
        if !stale_files(&dir, skill).is_empty() {
            return SkillState::Outdated;
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

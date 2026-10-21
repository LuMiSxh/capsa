// SPDX-License-Identifier: MPL-2.0
//! Owns the git guard: the agent must not write git hooks or config, which run code outside the
//! sandbox the next time a person uses git. Repositories made during the run stay writable.

mod config;

use anyhow::{Context, Result, bail};
use config::{config_findings, neutralise};
use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Component, Path, PathBuf};

/// Directories that are not searched for repositories (build output and dependencies).
const SKIP_DIRS: &[&str] = &["target", "node_modules", ".build"];
/// How many directories the walk visits before it stops.
const MAX_DIRS: usize = 50_000;
/// How many git directories get concrete entries; more fall back to globs.
const MAX_GIT_DIRS: usize = 256;
/// How deep `modules/` directories nest.
const MAX_MODULE_DEPTH: usize = 8;

/// Characters that make srt read a path as a glob.
const GLOB_CHARS: &[char] = &['*', '?', '[', ']', '{', '}'];

/// Holds the git directories and `.git` files found under a directory.
#[derive(Debug, Default)]
struct Found {
    /// `.git` directories and the git directories under their `modules/`.
    git_dirs: Vec<PathBuf>,
    /// `.git` entries that are not directories (worktree and submodule pointers, symlinks).
    git_links: Vec<PathBuf>,
    /// Directories outside any `.git` that hold `HEAD`, `objects/` and `refs/`: the shape of
    /// a bare repository, which git opens when it is the working directory.
    bare_dirs: Vec<PathBuf>,
    /// The walk or the list hit a cap.
    truncated: bool,
}

/// Walks `cwd` without following symlinks and skipping `SKIP_DIRS`.
fn scan(cwd: &Path, max_dirs: usize, max_git: usize) -> Found {
    let mut found = Found::default();
    let mut stack = vec![cwd.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited > max_dirs {
            found.truncated = true;
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let (mut head, mut objects, mut refs) = (false, false, false);
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let path = entry.path();
            head |= name == "HEAD" && !kind.is_dir();
            objects |= name == "objects" && kind.is_dir();
            refs |= name == "refs" && kind.is_dir();
            if name == ".git" {
                if kind.is_dir() {
                    found.git_dirs.push(path.clone());
                    collect_modules(&path.join("modules"), 0, &mut found.git_dirs);
                } else {
                    found.git_links.push(path);
                }
            } else if kind.is_dir() && !SKIP_DIRS.iter().any(|skip| name == *skip) {
                stack.push(path);
            }
        }
        if head && objects && refs {
            found.bare_dirs.push(dir);
        }
    }
    if found.git_dirs.len() > max_git {
        found.truncated = true;
    }
    found
}

/// Collects the git directories below a `modules/` directory. Submodule names may contain
/// slashes,
/// so a directory without `HEAD` is searched further.
fn collect_modules(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_MODULE_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path();
        if path.join("HEAD").symlink_metadata().is_ok() {
            out.push(path.clone());
            collect_modules(&path.join("modules"), depth + 1, out);
        } else {
            collect_modules(&path, depth + 1, out);
        }
    }
}

/// Returns the identity of a file-system entry, which survives a rename but not a replacement.
fn identity(path: &Path) -> Option<(u64, u64)> {
    path.symlink_metadata()
        .ok()
        .map(|meta| (meta.dev(), meta.ino()))
}

/// Holds the repositories under the working directory before the run.
#[derive(Debug)]
pub struct Start {
    cwd: PathBuf,
    ids: HashSet<(u64, u64)>,
    bare: HashSet<PathBuf>,
    found: Found,
}

impl Start {
    /// Lists the repositories under `cwd`.
    pub fn capture(cwd: &Path) -> Self {
        Self::capture_with(cwd, MAX_DIRS, MAX_GIT_DIRS)
    }

    fn capture_with(cwd: &Path, max_dirs: usize, max_git: usize) -> Self {
        let found = scan(cwd, max_dirs, max_git);
        let ids = found
            .git_dirs
            .iter()
            .chain(&found.git_links)
            .filter_map(|path| identity(path))
            .collect();
        Self {
            cwd: cwd.to_path_buf(),
            ids,
            bare: found.bare_dirs.iter().cloned().collect(),
            found,
        }
    }

    /// `deny_write` entries: `hooks` and `config` of every git directory and bare-shaped
    /// directory, and every `.git` file itself (it names the git directory). A capped walk or
    /// a path with glob
    /// characters adds `<cwd>/**/.git/...` globs, which also cover repositories made later.
    pub fn deny_entries(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut globs = self.found.truncated;
        let mut add = |path: PathBuf| match path.to_str() {
            Some(text) if !text.contains(GLOB_CHARS) => out.push(text.to_string()),
            _ => globs = true,
        };
        for dir in &self.found.git_dirs {
            add(dir.join("hooks"));
            add(dir.join("config"));
        }
        for bare in &self.found.bare_dirs {
            let mut modules = Vec::new();
            collect_modules(&bare.join("modules"), 0, &mut modules);
            for dir in std::iter::once(bare).chain(&modules) {
                add(dir.join("hooks"));
                add(dir.join("config"));
            }
        }
        for link in &self.found.git_links {
            add(link.clone());
        }
        let cwd = self.cwd.display();
        // Repositories inside skipped directories were not listed: their config stays read-only.
        for skip in SKIP_DIRS {
            out.push(format!("{cwd}/**/{skip}/**/.git/config"));
        }
        if globs {
            for tail in ["hooks", "config", "modules/*/hooks", "modules/*/config"] {
                out.push(format!("{cwd}/**/.git/{tail}"));
            }
        }
        out
    }

    /// Returns a warning when the walk was capped, else `None`.
    pub fn warning(&self) -> Option<String> {
        self.found.truncated.then(|| {
            format!(
                "warning: {} has more than {MAX_DIRS} directories or {MAX_GIT_DIRS} repositories; \
                 every .git/hooks and .git/config under it is read-only for this run, new repositories too",
                self.cwd.display()
            )
        })
    }

    fn is_new(&self, path: &Path) -> bool {
        identity(path).is_some_and(|id| !self.ids.contains(&id))
    }

    /// Returns `target` without `.` and `..` when it lies inside the working directory and
    /// every component from there on is a real directory, never a link.
    fn inside_dir(&self, target: &Path) -> Option<PathBuf> {
        let mut clean = PathBuf::new();
        for part in target.components() {
            match part {
                Component::ParentDir => {
                    clean.pop();
                }
                Component::CurDir => {}
                other => clean.push(other),
            }
        }
        let mut walk = self.cwd.clone();
        for part in clean.strip_prefix(&self.cwd).ok()?.components() {
            walk.push(part);
            if !walk.symlink_metadata().ok()?.is_dir() {
                return None;
            }
        }
        Some(clean)
    }

    /// Scans for git directories that did not exist at the start. Their hooks move to
    /// `<home>/Library/Application Support/velum/quarantine/<run_id>/`; config entries
    /// that run code are only reported. Returns the warning lines.
    pub fn check(&self, home: &str, run_id: &str) -> Vec<String> {
        let now = scan(&self.cwd, MAX_DIRS, usize::MAX);
        let mut lines = Vec::new();
        if now.truncated {
            lines.push(format!(
                "warning: the scan for new git repositories under {} stopped early",
                self.cwd.display()
            ));
        }
        let new_bare: Vec<String> = now
            .bare_dirs
            .iter()
            .filter(|dir| !self.bare.contains(*dir))
            .map(|dir| dir.display().to_string())
            .collect();
        if !new_bare.is_empty() {
            lines.push(format!(
                "warning: directories that look like bare git repositories (HEAD, objects/, refs/) \
                 appeared during the run: {}\nhint: run `git config --global safe.bareRepository explicit` \
                 so git does not open a bare repository found in the working directory",
                new_bare.join(", ")
            ));
        }
        let mut fresh: Vec<PathBuf> = now
            .git_dirs
            .into_iter()
            .filter(|dir| self.is_new(dir))
            .collect();
        for link in now.git_links.iter().filter(|link| self.is_new(link)) {
            lines.push(format!(
                "warning: new .git file or link created during the run: {}",
                link.display()
            ));
            let Some(target) = gitdir_target(link) else {
                continue;
            };
            match self.inside_dir(&target) {
                Some(dir) if self.is_new(&dir) => fresh.push(dir),
                Some(_) => {}
                None => lines.push(format!(
                    "warning: {} names {}, which is outside the working directory or reached \
                     through a link; its hooks and config were not checked",
                    link.display(),
                    target.display()
                )),
            }
        }
        let (mut hooks, mut configs) = (Vec::new(), Vec::new());
        let mut removed = Vec::new();
        for dir in &fresh {
            hooks.extend(offending_hooks(dir));
            match neutralise(dir) {
                Ok(done) => removed.extend(done),
                Err(err) => lines.push(format!(
                    "warning: could not remove code-running git config entries, edit them by hand: {err:#}"
                )),
            }
            configs.extend(config_findings(dir));
        }
        if !removed.is_empty() {
            lines.push(format!(
                "warning: removed git config entries that hook every git command to a program, \
                 created during the run: {}",
                removed.join(", ")
            ));
        }
        if !hooks.is_empty() {
            lines.extend(quarantine(home, run_id, &hooks));
        }
        if !configs.is_empty() {
            lines.push(format!(
                "warning: git config entries created during the run that run code (review them): {}",
                configs.join(", ")
            ));
        }
        lines
    }
}

/// Returns the path a `.git` file names after `gitdir:`, relative ones resolved against the
/// file's directory.
fn gitdir_target(link: &Path) -> Option<PathBuf> {
    let mut text = String::new();
    fs::File::open(link)
        .ok()?
        .take(4096)
        .read_to_string(&mut text)
        .ok()?;
    let target = text.lines().find_map(|line| line.strip_prefix("gitdir:"))?;
    Some(link.parent()?.join(target.trim()))
}

/// Returns the hook entries of `git_dir` that are not `.sample` files, or the `hooks`
/// directory itself when it is a link.
fn offending_hooks(git_dir: &Path) -> Vec<PathBuf> {
    let hooks = git_dir.join("hooks");
    let Ok(meta) = hooks.symlink_metadata() else {
        return Vec::new();
    };
    if !meta.is_dir() {
        return vec![hooks];
    }
    let Ok(entries) = fs::read_dir(&hooks) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".sample"))
        .map(|entry| entry.path())
        .collect()
}

/// Moves `files` into the run's quarantine directory and returns the warning lines.
fn quarantine(home: &str, run_id: &str, files: &[PathBuf]) -> Vec<String> {
    let dir = Path::new(home)
        .join("Library/Application Support/velum/quarantine")
        .join(run_id);
    let made = fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir);
    let mut lines = Vec::new();
    let mut moved = Vec::new();
    let mut failed = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let dest = dir.join(format!("{index}-{name}"));
        if made.is_ok() && move_aside(file, &dest).is_ok() {
            moved.push(format!("{} (was {})", dest.display(), file.display()));
        } else {
            failed.push(file.display().to_string());
        }
    }
    if !moved.is_empty() {
        lines.push(format!(
            "warning: git hooks created during the run would run code outside the sandbox; moved to {}: {}",
            dir.display(),
            moved.join(", ")
        ));
    }
    if !failed.is_empty() {
        lines.push(format!(
            "warning: could not quarantine these git hooks, remove them by hand: {}",
            failed.join(", ")
        ));
    }
    lines
}

/// Renames `from` to `to`; when that fails (across file systems) it copies a regular file to
/// disk and removes the source. Anything else is an error and stays where it is.
fn move_aside(from: &Path, to: &Path) -> Result<()> {
    move_aside_with(from, to, |a, b| fs::rename(a, b))
}

fn move_aside_with(
    from: &Path,
    to: &Path,
    rename: impl Fn(&Path, &Path) -> io::Result<()>,
) -> Result<()> {
    if rename(from, to).is_ok() {
        return Ok(());
    }
    if !from.symlink_metadata()?.is_file() {
        bail!(
            "{} is not a regular file and cannot be copied",
            from.display()
        );
    }
    fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    fs::File::open(to)?.sync_all()?;
    fs::remove_file(from)?;
    Ok(())
}

#[cfg(test)]
mod tests;

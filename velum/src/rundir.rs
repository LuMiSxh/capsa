// SPDX-License-Identifier: MPL-2.0
//! Owns the per-run private directory: generated srt settings and wrapper scripts.
//!
//! It lives under `~/Library/Application Support/velum/run/`, not in `TMPDIR`:
//! `/private/tmp` and `/private/var/folders` are write-allowed for the agent, so
//! anything there could be replaced between velum writing it and srt or the shell
//! reading it. The parent is also in the default `deny_write`.

use crate::policy::PolicyError;
use crate::proc;
use crate::writable;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Below `$HOME`: where per-run directories are made.
const BASE: &str = "Library/Application Support/velum/run";

/// A run directory older than this is left over from a run that was killed.
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Removes run directories under `<home>/<BASE>` that were last modified before `now - max_age`.
/// Only plain directories named `<pid>-<hex>` are touched, never through a symlink and never
/// while a process with that pid exists. Returns how many were removed.
pub fn remove_stale(home: &str, now: SystemTime, max_age: Duration) -> usize {
    let base = Path::new(home).join(BASE);
    if ensure_real_dir(&base).is_err() {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(&base) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let owner = name.split_once('-').and_then(|(pid, hex)| {
            let plain = !pid.is_empty()
                && pid.bytes().all(|b| b.is_ascii_digit())
                && !hex.is_empty()
                && hex.bytes().all(|b| b.is_ascii_hexdigit());
            plain.then(|| pid.parse::<i32>().ok())
        });
        let run_like = owner.is_some_and(|pid| !pid.is_some_and(proc::pid_alive));
        let path = entry.path();
        let old = path
            .symlink_metadata()
            .ok()
            .filter(|meta| meta.is_dir())
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if run_like && old && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Holds a fresh, exclusively created, mode 0700 directory, removed when dropped
/// (so also on every error path of the run).
#[derive(Debug)]
pub struct RunDir {
    path: PathBuf,
}

impl RunDir {
    /// Creates `<home>/Library/Application Support/velum/run/<pid>-<nanos>`.
    /// Fails when the name exists, and never follows a symlink at the end.
    pub fn create(home: &str) -> Result<Self> {
        let base = Path::new(home).join(BASE);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&base)
            .with_context(|| format!("creating {}", base.display()))?;
        ensure_real_dir(&base)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = base.join(format!("{}-{nanos:x}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        let dir = Self { path };
        ensure_real_dir(&dir.path)?;
        std::fs::set_permissions(&dir.path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod {}", dir.path.display()))?;
        Ok(dir)
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the directory's name, which identifies the run.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Refuses when any `allowWrite` entry of the merged settings covers the run directory.
    pub fn ensure_not_writable(&self, settings: &Value) -> Result<(), PolicyError> {
        match writable::allowed_by(settings, &self.path) {
            None => Ok(()),
            Some(entry) => Err(PolicyError::RunDirWritable {
                run_dir: self.path.clone(),
                entry,
            }),
        }
    }

    /// Creates the sub-directory `name` (mode 0700) and returns its path.
    pub fn make_dir(&self, name: &str) -> Result<PathBuf> {
        let path = self.child(name)?;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        Ok(path)
    }

    /// Writes a new file `name` (a relative path) with `mode`; fails when it exists.
    pub fn write_file(&self, name: &str, bytes: &[u8], mode: u32) -> Result<PathBuf> {
        let path = self.child(name)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("writing {}", path.display()))?;
        // The umask may have cleared bits of `mode`.
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("chmod {}", path.display()))?;
        Ok(path)
    }

    /// Returns the path of the entry `name` (a plain file name) in the directory.
    pub fn child(&self, name: &str) -> Result<PathBuf> {
        let plain = Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
        if name.is_empty() || !plain {
            bail!("invalid run directory entry {name:?}");
        }
        Ok(self.path.join(name))
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Checks that `path` is a directory and not a symlink.
fn ensure_real_dir(path: &Path) -> Result<()> {
    let meta = path
        .symlink_metadata()
        .with_context(|| format!("inspecting {}", path.display()))?;
    if !meta.is_dir() {
        bail!("{} is not a plain directory (symlink?)", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use serde_json::json;

    fn home(scratch: &Scratch) -> String {
        scratch.path().to_str().unwrap().to_string()
    }

    #[test]
    fn creates_a_private_unique_directory_and_removes_it_on_drop() {
        let scratch = Scratch::new("rundir-create");
        let a = RunDir::create(&home(&scratch)).unwrap();
        let b = RunDir::create(&home(&scratch)).unwrap();
        assert_ne!(a.path(), b.path());
        let mode = std::fs::metadata(a.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let kept = a.path().to_path_buf();
        drop(a);
        assert!(!kept.exists());
        assert!(b.path().exists());
    }

    #[test]
    fn files_are_created_exclusively_with_private_modes() {
        let scratch = Scratch::new("rundir-files");
        let dir = RunDir::create(&home(&scratch)).unwrap();
        let file = dir.write_file("settings.json", b"{}", 0o600).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(dir.write_file("settings.json", b"{}", 0o600).is_err());
        dir.make_dir("bin").unwrap();
        let script = dir.write_file("bin/swift", b"#!/bin/sh\n", 0o700).unwrap();
        assert_eq!(
            std::fs::metadata(&script).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(dir.make_dir("bin").is_err());
    }

    #[test]
    fn files_never_follow_a_planted_symlink() {
        let scratch = Scratch::new("rundir-symlink");
        let dir = RunDir::create(&home(&scratch)).unwrap();
        let victim = scratch.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join("settings.json")).unwrap();
        assert!(dir.write_file("settings.json", b"x", 0o600).is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    }

    #[test]
    fn names_must_stay_inside_the_directory() {
        let scratch = Scratch::new("rundir-names");
        let dir = RunDir::create(&home(&scratch)).unwrap();
        for bad in ["", "../x", "/abs", "a/../b"] {
            assert!(dir.write_file(bad, b"", 0o600).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_symlinked_base_is_refused() {
        let scratch = Scratch::new("rundir-base-link");
        let home = scratch.path().join("home");
        let elsewhere = scratch.path().join("elsewhere");
        std::fs::create_dir_all(home.join("Library/Application Support/velum")).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.join(BASE)).unwrap();
        assert!(RunDir::create(home.to_str().unwrap()).is_err());
    }

    #[test]
    fn stale_run_dirs_are_removed_and_fresh_or_foreign_ones_are_not() {
        let scratch = Scratch::new("rundir-stale");
        let h = home(&scratch);
        let live = RunDir::create(&h).unwrap();
        let base = live.path().parent().unwrap().to_path_buf();
        let mut finished = std::process::Command::new("true").spawn().unwrap();
        finished.wait().unwrap();
        let old = base.join(format!("{}-abc", finished.id()));
        let alive = base.join(format!("{}-abc", std::process::id()));
        std::fs::create_dir(&alive).unwrap();
        let named_oddly = base.join("keep-me");
        let outside = scratch.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("file"), "x").unwrap();
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&named_oddly).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("2-def")).unwrap();
        assert_eq!(remove_stale(&h, SystemTime::now(), STALE_AFTER), 0);
        assert!(live.path().exists());
        // Two days on, only the directory of the finished process counts as stale.
        let later = SystemTime::now() + Duration::from_secs(2 * 24 * 60 * 60);
        assert_eq!(remove_stale(&h, later, STALE_AFTER), 1);
        assert!(!old.exists());
        assert!(live.path().exists());
        assert!(alive.exists());
        assert!(named_oddly.exists());
        assert!(outside.join("file").exists());
        assert!(base.join("2-def").symlink_metadata().is_ok());
    }

    #[test]
    fn refuses_when_an_allow_write_entry_covers_the_run_dir() {
        let scratch = Scratch::new("rundir-covered");
        let dir = RunDir::create(&home(&scratch)).unwrap();
        let covering = |entry: String| json!({"filesystem": {"allowWrite": [entry]}});
        let h = home(&scratch);
        for entry in [
            h.clone(),
            format!("{h}/Library"),
            format!("{h}/Library/Application Support/velum"),
            dir.path().display().to_string(),
            format!("{h}/**"),
            "/".to_string(),
        ] {
            let err = dir
                .ensure_not_writable(&covering(entry.clone()))
                .unwrap_err();
            assert!(matches!(err, PolicyError::RunDirWritable { .. }), "{entry}");
        }
        for entry in [
            format!("{h}/Library/Caches"),
            "/private/tmp/velum".to_string(),
        ] {
            assert!(dir.ensure_not_writable(&covering(entry)).is_ok());
        }
    }
}

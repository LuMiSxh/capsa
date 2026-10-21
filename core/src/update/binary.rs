// SPDX-License-Identifier: MPL-2.0
//! Replaces and removes the running binary without leaving a half-written file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::UpdateError;

/// Returns the running binary with every symlink resolved, so the real file is the one changed.
pub(super) fn resolve() -> Result<PathBuf, UpdateError> {
    std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(UpdateError::Locate)
}

/// Removes the `.old` file a Windows update leaves, because a running `.exe` cannot be
/// overwritten and is renamed aside instead.
pub(super) fn forget_old(exe: &Path) {
    let _ = fs::remove_file(exe.with_extension("old"));
}

/// Refuses a binary whose file name is not `name`.
pub(super) fn ensure_named(exe: &Path, name: &'static str) -> Result<(), UpdateError> {
    if exe.file_stem().is_some_and(|stem| stem == name) {
        Ok(())
    } else {
        Err(UpdateError::NotNamed {
            path: exe.to_path_buf(),
            name,
        })
    }
}

/// Refuses anything but a regular file; `exe` is already resolved, so a link here means it
/// changed since.
fn ensure_plain(exe: &Path) -> Result<(), UpdateError> {
    let write = |source| UpdateError::Write {
        path: exe.to_path_buf(),
        source,
    };
    let meta = fs::symlink_metadata(exe).map_err(write)?;
    if meta.file_type().is_symlink() {
        return Err(UpdateError::Symlink {
            path: exe.to_path_buf(),
        });
    }
    if !meta.is_file() {
        return Err(UpdateError::NotAFile {
            path: exe.to_path_buf(),
        });
    }
    Ok(())
}

/// Replaces `exe` with `new`: the bytes go to a fresh file in the same directory, are synced,
/// then renamed over `exe`, so readers see the old or the new binary and never a partial one.
/// A running Windows binary is first renamed to `.old`, which the next update removes. A binary
/// not named `name` is refused, so a renamed copy is not overwritten with the release.
pub(super) fn replace(exe: &Path, name: &'static str, new: &[u8]) -> Result<(), UpdateError> {
    ensure_named(exe, name)?;
    ensure_plain(exe)?;
    let write = |path: &Path| {
        let path = path.to_path_buf();
        move |source| UpdateError::Write { path, source }
    };
    let name = exe.file_name().unwrap_or_default().to_string_lossy();
    let tmp = exe.with_file_name(format!(".{name}.update-{}", std::process::id()));
    let mut file = create_new(&tmp).map_err(write(&tmp))?;
    if let Err(err) = file.write_all(new).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&tmp);
        return Err(write(&tmp)(err));
    }
    drop(file);
    if let Err(err) = swap(exe, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(write(exe)(err));
    }
    sync_dir(exe);
    Ok(())
}

/// Creates a file that must not exist yet, so a planted file or link is never followed or
/// removed.
fn create_new(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o755);
    options.open(path)
}

#[cfg(not(windows))]
fn swap(exe: &Path, tmp: &Path) -> std::io::Result<()> {
    fs::rename(tmp, exe)
}

#[cfg(windows)]
fn swap(exe: &Path, tmp: &Path) -> std::io::Result<()> {
    let old = exe.with_extension("old");
    fs::rename(exe, &old)?;
    fs::rename(tmp, exe).inspect_err(|_| {
        let _ = fs::rename(&old, exe);
    })
}

/// Syncs the directory of `exe`, so the rename survives a crash; best effort.
fn sync_dir(exe: &Path) {
    #[cfg(unix)]
    if let Some(dir) = exe.parent()
        && let Ok(dir) = fs::File::open(dir)
    {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = exe;
}

/// Removes `exe`. A running Windows binary cannot be deleted, so it is renamed to `.old`.
pub(super) fn remove(exe: &Path) -> Result<(), UpdateError> {
    ensure_plain(exe)?;
    #[cfg(windows)]
    let result = fs::rename(exe, exe.with_extension("old"));
    #[cfg(not(windows))]
    let result = fs::remove_file(exe);
    result.map_err(|source| UpdateError::Write {
        path: exe.to_path_buf(),
        source,
    })
}

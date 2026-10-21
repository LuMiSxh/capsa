// SPDX-License-Identifier: MPL-2.0
//! Owns the directories that the sandbox can reach, made without following links.

use crate::proc;
use anyhow::{Context, Result};
use capsa_core::hint::Hinted;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;

/// Makes `path` a mode 0700 directory when it does not exist. When it does, it must be a
/// real directory (not a symlink) owned by the current user. The parent must exist.
pub fn ensure_private_dir(path: &Path) -> Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err).with_context(|| format!("creating {}", path.display())),
    }
    let meta = path
        .symlink_metadata()
        .with_context(|| format!("inspecting {}", path.display()))?;
    if !meta.is_dir() {
        return Err(Hinted::new(
            format!(
                "{} is not a plain directory (symlink or file)",
                path.display()
            ),
            format!("remove {}, then run velum again", path.display()),
        )
        .into());
    }
    if meta.uid() != proc::euid() {
        return Err(Hinted::new(
            format!("{} is owned by another user", path.display()),
            format!("remove or rename {}, then run velum again", path.display()),
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn creates_a_missing_directory_and_accepts_an_own_one() {
        let scratch = Scratch::new("safedir-ok");
        let dir = scratch.path().join("claude");
        ensure_private_dir(&dir).unwrap();
        assert!(dir.is_dir());
        ensure_private_dir(&dir).unwrap();
    }

    #[test]
    fn refuses_a_symlink_and_a_file() {
        let scratch = Scratch::new("safedir-bad");
        let real = scratch.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = scratch.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = ensure_private_dir(&link).unwrap_err();
        assert!(capsa_core::hint::find_hint(&err).is_some());
        let file = scratch.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(ensure_private_dir(&file).is_err());
    }
}

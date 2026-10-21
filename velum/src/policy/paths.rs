// SPDX-License-Identifier: MPL-2.0
//! Owns the checks on paths: the policy file location, `HOME`, the working directory and entries.

use super::PolicyError;
use std::path::{Path, PathBuf};

/// The policy file read when `--policy` is not given.
pub fn default_path(home: &str) -> PathBuf {
    Path::new(home).join(".config/velum/policy.toml")
}

/// Places under `$HOME` that hold credentials, tool configuration or velum's own files;
/// no run may start in one, or inside one.
const PROTECTED_DIRS: &[&str] = &[
    ".ssh", ".aws", ".gnupg", ".config", ".local", "Library", ".claude", ".cargo", ".rustup",
];

/// Refuses a working directory that would be added to `allowWrite` wholesale although it
/// is too broad (`/`, `$HOME` or an ancestor of it) or is, or lies inside, one of
/// `PROTECTED_DIRS`.
pub fn check_cwd(cwd: &Path, home: &str) -> Result<(), PolicyError> {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let home_path = Path::new(home);
    let cwds = [cwd.to_path_buf(), real(cwd)];
    let homes = [home_path.to_path_buf(), real(home_path)];
    let refuse = |reason| {
        Err(PolicyError::CwdRefused {
            cwd: cwd.to_path_buf(),
            reason,
        })
    };
    for dir in &cwds {
        if dir.parent().is_none() {
            return refuse("it is the file-system root");
        }
        for home in &homes {
            if home.starts_with(dir) {
                return refuse("it is your home directory or contains it");
            }
            if PROTECTED_DIRS
                .iter()
                .any(|own| dir.starts_with(home.join(own)))
            {
                return refuse("it is, or is inside, a credentials, config or tool directory");
            }
        }
    }
    Ok(())
}

/// Which list an entry belongs to; deny lists may also hold `**/name` globs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Allow,
    Deny,
}

/// Checks that `HOME` is non-empty, absolute and free of `..`.
pub fn validate_home(home: &str) -> Result<&str, PolicyError> {
    let bad = |reason| {
        Err(PolicyError::BadHome {
            home: home.to_string(),
            reason,
        })
    };
    if home.is_empty() {
        return bad("empty");
    }
    if !home.starts_with('/') {
        return bad("not an absolute path");
    }
    if has_parent_dir(home) {
        return bad("contains `..`");
    }
    Ok(home)
}

fn has_parent_dir(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|part| part == std::path::Component::ParentDir)
}

/// Expands a leading `~` to `home` and checks the result: absolute (or, in a deny
/// list, `**/` followed by a relative name) and without `..`. `~user` is refused.
pub fn expand_entry(entry: &str, home: &str, kind: Entry) -> Result<String, PolicyError> {
    let invalid = |reason| {
        Err(PolicyError::InvalidPath {
            entry: entry.to_string(),
            reason,
        })
    };
    let expanded = match entry.strip_prefix('~') {
        Some("") => home.to_string(),
        Some(rest) if rest.starts_with('/') => format!("{home}{rest}"),
        Some(_) => return invalid("`~user` forms are not supported"),
        None => entry.to_string(),
    };
    if has_parent_dir(&expanded) {
        return invalid("contains `..`");
    }
    let glob = kind == Entry::Deny && expanded.starts_with("**/");
    if !expanded.starts_with('/') && !glob {
        return invalid("not an absolute path");
    }
    Ok(expanded)
}

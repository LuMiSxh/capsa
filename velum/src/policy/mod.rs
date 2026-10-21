// SPDX-License-Identifier: MPL-2.0
//! Owns the sandbox policy: the merged defaults and file, and the checks on its paths.

mod builtin;
mod error;
mod file;
mod paths;
#[cfg(test)]
mod tests;

use std::path::Path;

pub use builtin::DEFAULT_POLICY_TOML;
pub use error::PolicyError;
pub use paths::{Entry, check_cwd, default_path, expand_entry, validate_home};

/// Holds the merged policy: built-in defaults extended by the policy file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub write: Vec<String>,
    pub deny_read: Vec<String>,
    pub deny_write: Vec<String>,
    pub env_pass: Vec<String>,
    /// Exact names of secret-looking variables the user chose to pass (`env.pass_secret`).
    pub env_pass_secret: Vec<String>,
    pub net_allow: Vec<String>,
}

impl Policy {
    /// Loads the policy file; a missing file means built-in defaults only.
    pub fn load(path: &Path, cwd: &Path) -> Result<Self, PolicyError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(cwd, &text).map_err(|err| PolicyError::Parse {
                path: path.to_path_buf(),
                message: format!("{err:#}"),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::builtin(cwd)),
            Err(source) => Err(PolicyError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }
}

/// Matches an env var name against a pattern; a trailing `*` is a prefix wildcard.
pub fn env_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

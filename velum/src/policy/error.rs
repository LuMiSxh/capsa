// SPDX-License-Identifier: MPL-2.0
//! Owns the errors of the policy: a bad file, a bad path or home, a refused working directory.

use capsa_core::hint::Hint;
use std::borrow::Cow;
use std::path::PathBuf;
use thiserror::Error;

/// Records why the policy file cannot be used. Each variant names the file.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// The file exists but cannot be read.
    #[error("reading {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid policy TOML.
    #[error("parsing {}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    /// A file-system entry is not an absolute path without `..` after `~` expansion.
    #[error("invalid path {entry:?} in the policy: {reason}")]
    InvalidPath { entry: String, reason: &'static str },
    /// `HOME` is empty, relative or contains `..`.
    #[error("HOME is {home:?}: {reason}")]
    BadHome { home: String, reason: &'static str },
    /// The working directory is too broad or holds velum's own files.
    #[error("refusing to run in {}: {reason}", cwd.display())]
    CwdRefused { cwd: PathBuf, reason: &'static str },
    /// The per-run directory is covered by a write-allowed path.
    #[error("the run directory {} is writable by the sandbox (allowed by {entry})", run_dir.display())]
    RunDirWritable { run_dir: PathBuf, entry: String },
    /// A file velum trusts (srt, the real claude, velum itself) is writable by the sandbox.
    #[error("{what} {} is writable by the sandbox (allowed by {entry})", path.display())]
    TrustedWritable {
        what: &'static str,
        path: PathBuf,
        entry: String,
    },
}

impl Hint for PolicyError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        Some(match self {
            Self::Read { path, .. } | Self::Parse { path, .. } => Cow::Owned(format!(
                "fix or remove {}, then run `velum self doctor`",
                path.display()
            )),
            Self::InvalidPath { .. } => Cow::Borrowed(
                "use an absolute path or one starting with `~/` (deny lists also accept `**/name`), without `..`",
            ),
            Self::BadHome { .. } => Cow::Borrowed("set HOME to your absolute home directory"),
            Self::CwdRefused { .. } => {
                Cow::Borrowed("cd into a project directory and run velum there")
            }
            Self::RunDirWritable { .. } => Cow::Borrowed(
                "remove that entry from `fs.write` in the policy file; velum keeps its settings outside every writable path",
            ),
            Self::TrustedWritable { .. } => Cow::Borrowed(
                "install it outside every writable path (for example in ~/.cargo/bin or /usr/local/bin), or run from another directory",
            ),
        })
    }
}

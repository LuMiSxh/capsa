// SPDX-License-Identifier: MPL-2.0
//! The errors of updating and uninstalling, each with the fix when there is one.

use std::borrow::Cow;
use std::path::PathBuf;

use super::REPO;
use crate::hint::Hint;

/// Why an update or an uninstall failed.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("the repository {REPO} is private and no GitHub token was found")]
    NoToken,
    #[error("no release build for {os}-{arch}")]
    UnsupportedPlatform {
        os: &'static str,
        arch: &'static str,
    },
    #[error("no {tool} release found in {REPO}")]
    NoRelease { tool: &'static str },
    #[error("release {tag} has no asset {name}")]
    MissingAsset { tag: String, name: String },
    #[error("cannot reach {url}")]
    Http {
        url: String,
        #[source]
        source: Box<ureq::Error>,
    },
    #[error("checksum mismatch: expected {want}, got {got}")]
    ChecksumMismatch { want: String, got: String },
    #[error("{bin} missing from the archive")]
    MissingInArchive { bin: String },
    #[error("cannot read the archive")]
    Archive(#[source] std::io::Error),
    #[error("cannot read the archive")]
    Zip(#[source] zip::result::ZipError),
    #[error("cannot locate the running binary")]
    Locate(#[source] std::io::Error),
    #[error("{} is a symbolic link; refusing to replace it", path.display())]
    Symlink { path: PathBuf },
    #[error("{} is not a regular file", path.display())]
    NotAFile { path: PathBuf },
    #[error("{} is not named {name}; refusing to remove it", path.display())]
    NotNamed { path: PathBuf, name: &'static str },
    #[error("cannot write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Hint for UpdateError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(match self {
            Self::NoToken => "set GITHUB_TOKEN, or run `gh auth login`",
            Self::UnsupportedPlatform { .. } => {
                "build it from source: cargo install --git https://github.com/LuMiSxh/capsa <tool>"
            }
            Self::Symlink { .. } | Self::NotAFile { .. } => {
                "run the tool from its installed path, or use the installer"
            }
            Self::NotNamed { .. } => "run the installed binary itself, not a renamed copy",
            Self::Write { .. } => "rerun with permission to write there, or use the installer",
            Self::ChecksumMismatch { .. } => "nothing was changed; try again or use the installer",
            _ => return None,
        }))
    }
}

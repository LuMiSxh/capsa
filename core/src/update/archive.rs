// SPDX-License-Identifier: MPL-2.0
//! Checks a release archive against its checksum and takes the binary out of it.

use std::io::Read;

use sha2::{Digest, Sha256};

use super::{Tool, UpdateError};

/// Returns the archive extension and binary file name of `tool` on this platform.
pub(super) fn kind(tool: &Tool) -> (&'static str, String) {
    if cfg!(windows) {
        ("zip", format!("{}.exe", tool.name))
    } else {
        ("tar.gz", tool.name.to_owned())
    }
}

/// Checks `data` against a `HEX  NAME` checksum line.
pub(super) fn verify(data: &[u8], sum: &str) -> Result<(), UpdateError> {
    let want = sum
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let got: String = Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if want == got {
        Ok(())
    } else {
        Err(UpdateError::ChecksumMismatch { want, got })
    }
}

/// Returns the regular file named `bin` from a `tar.gz` or `zip` archive. A link or directory of
/// that name is not a binary, and an empty file is refused, so neither is ever installed.
pub(super) fn extract(archive: &[u8], ext: &str, bin: &str) -> Result<Vec<u8>, UpdateError> {
    let missing = || UpdateError::MissingInArchive {
        bin: bin.to_owned(),
    };
    let mut out = Vec::new();
    if ext == "zip" {
        let mut zip =
            zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(UpdateError::Zip)?;
        let mut file = zip.by_name(bin).map_err(|_| missing())?;
        let symlink = file.unix_mode().is_some_and(|m| m & 0o170_000 == 0o120_000);
        if file.is_dir() || symlink {
            return Err(missing());
        }
        file.read_to_end(&mut out).map_err(UpdateError::Archive)?;
        return non_empty(out, missing);
    }
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(UpdateError::Archive)? {
        let mut entry = entry.map_err(UpdateError::Archive)?;
        let named = entry
            .path()
            .map_err(UpdateError::Archive)?
            .file_name()
            .is_some_and(|n| n == bin);
        if named && entry.header().entry_type().is_file() {
            entry.read_to_end(&mut out).map_err(UpdateError::Archive)?;
            return non_empty(out, missing);
        }
    }
    Err(missing())
}

fn non_empty(out: Vec<u8>, missing: impl FnOnce() -> UpdateError) -> Result<Vec<u8>, UpdateError> {
    if out.is_empty() {
        Err(missing())
    } else {
        Ok(out)
    }
}

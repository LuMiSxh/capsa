// SPDX-License-Identifier: MPL-2.0
//! Owns the transcript walk: the `*.jsonl` files under a directory, hidden ones included.

use std::path::{Path, PathBuf};

/// Lists the `*.jsonl` files under `root`, sorted by path so the report does not depend on directory order.
///
/// Symbolic links are not followed, and directories that cannot be read are skipped.
pub fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let is_transcript = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".jsonl"));
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() && is_transcript {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

// SPDX-License-Identifier: MPL-2.0
//! Checks a patch's syntax before anything is written, then writes every changed file.

use super::files::{File, Files};
use crate::error::{EditError, Result};
use crate::parse::markers::{first_marker, has_markers, unbalanced};
use acus_syntax::{Lang, is_jsonc, syntax_errors};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Files the patch writes whose syntax gets worse: more error nodes than they had before. A file
/// that held conflict markers is checked only on the result; one that still holds them is not
/// checked at all. A patch may not leave an unmatched marker or add conflict markers where there
/// were none. A JSON file that already uses comments or trailing commas is checked as JSONC.
/// Nothing is written yet, so an error here fails the whole patch unless `allow_broken`.
pub(super) fn check_syntax(
    files: &Files,
    allow_broken: bool,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut broken = Vec::new();
    let mut unchecked = Vec::new();
    for (path, f) in files.map.iter().filter(|(_, f)| f.dirty) {
        let Some(lines) = &f.lines else {
            continue;
        };
        // The text as `commit` writes it, so the trailing newline counts as it does on disk.
        let mut text = lines.join("\n");
        if f.trailing_newline && !lines.is_empty() {
            text.push('\n');
        }
        let before = f.before.as_deref();
        let had_markers = before.is_some_and(has_markers);
        let was_unbalanced = before.is_some_and(|t| unbalanced(t).is_some());
        if let Some(line) = unbalanced(&text) {
            if !was_unbalanced && !allow_broken {
                return Err(EditError::UnmatchedConflict {
                    path: path.into(),
                    line,
                });
            }
            unchecked.push(path.clone());
            continue;
        }
        if !had_markers && has_markers(&text) {
            if !allow_broken {
                return Err(EditError::IntroducedConflict {
                    path: path.into(),
                    line: first_marker(&text).unwrap_or(1),
                });
            }
            broken.push(path.clone());
            continue;
        }
        if had_markers && has_markers(&text) {
            unchecked.push(path.clone());
            continue;
        }
        let Some(lang) = Lang::from_path(Path::new(path)) else {
            continue;
        };
        let jsonc = lang == Lang::Json && before.is_some_and(is_jsonc);
        let Some(new) = syntax_errors(lang, &text, jsonc).filter(|e| !e.is_empty()) else {
            continue;
        };
        let old = if had_markers {
            0
        } else {
            before
                .and_then(|t| syntax_errors(lang, t, jsonc))
                .map_or(0, |e| e.len())
        };
        if new.len() <= old {
            continue;
        }
        if !allow_broken {
            let Some(&at) = new.iter().min() else {
                continue;
            };
            let snippet: Vec<String> = (at.saturating_sub(1).max(1)..=(at + 1).min(lines.len()))
                .map(|n| format!("{n}\t{}", lines[n - 1]))
                .collect();
            return Err(EditError::BrokenSyntax {
                path: path.into(),
                line: at,
                snippet,
            });
        }
        broken.push(path.clone());
    }
    Ok((broken, unchecked))
}

/// Writes every dirty file to a temp file next to it, then renames them all into place.
/// Temp files are removed on any error (NamedTempFile's Drop); originals stay untouched
/// until the first rename.
pub(super) fn commit(root: &Path, map: BTreeMap<String, File>) -> Result<()> {
    let mut staged = Vec::new();
    let mut deletes = Vec::new();
    for (path, f) in map.into_iter().filter(|(_, f)| f.dirty) {
        let full = root.join(&path);
        let Some(lines) = f.lines else {
            if f.existed {
                deletes.push((path, full));
            }
            continue;
        };
        // A symlink is written through, so the rename replaces the file it points to and keeps the link.
        let full = write_target(&full).map_err(|source| EditError::Commit {
            path: path.clone(),
            step: "follow symlink",
            source,
        })?;
        let mut text = lines.join("\n");
        if f.trailing_newline && !lines.is_empty() {
            text.push('\n');
        }
        if f.crlf {
            text = text.replace('\n', "\r\n");
        }
        let dir = full
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(dir).map_err(|source| EditError::Commit {
            path: path.clone(),
            step: "create directory",
            source,
        })?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|source| EditError::Commit {
            path: path.clone(),
            step: "create temp file",
            source,
        })?;
        tmp.write_all(text.as_bytes())
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|source| EditError::Commit {
                path: path.clone(),
                step: "write",
                source,
            })?;
        if let Some(p) = f.perms {
            fs::set_permissions(tmp.path(), p).map_err(|source| EditError::Commit {
                path: path.clone(),
                step: "copy permissions",
                source,
            })?;
        }
        staged.push((path, full, tmp));
    }
    let mut done: Vec<String> = Vec::new();
    for (path, full, tmp) in staged {
        if let Err(e) = tmp.persist(&full) {
            return Err(EditError::Rename {
                path,
                error: e.error,
                done: done.clone(),
            });
        }
        done.push(path);
    }
    for (path, full) in deletes {
        if let Err(e) = fs::remove_file(&full) {
            return Err(EditError::Delete {
                path,
                error: e,
                done: done.clone(),
            });
        }
        done.push(path);
    }
    Ok(())
}

/// The path a write should go to: the target of a symlink, or `full` itself.
fn write_target(full: &Path) -> std::io::Result<std::path::PathBuf> {
    if fs::symlink_metadata(full).is_ok_and(|m| m.file_type().is_symlink()) {
        return fs::canonicalize(full);
    }
    Ok(full.to_path_buf())
}

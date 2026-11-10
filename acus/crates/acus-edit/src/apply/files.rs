// SPDX-License-Identifier: MPL-2.0
//! The files a patch edits: read on first use, kept in memory until `commit` writes them.

use crate::error::{EditError, Result};
use std::collections::BTreeMap;
use std::fs::{self, Permissions};
use std::path::Path;

/// A file as the patch sees it; `lines: None` means deleted or not existing.
pub(super) struct File {
    pub(super) lines: Option<Vec<String>>,
    /// The text as read from disk, LF line ends; `None` for a file that did not exist.
    pub(super) before: Option<String>,
    pub(super) crlf: bool,
    pub(super) trailing_newline: bool,
    pub(super) perms: Option<Permissions>,
    pub(super) existed: bool,
    pub(super) dirty: bool,
    /// Edited (0-based start, length) ranges in the current `lines`.
    pub(super) edits: Vec<(usize, usize)>,
}

impl File {
    /// Splices `new` over `at..at + old` and keeps earlier edit ranges pointing at their lines.
    #[expect(clippy::expect_used, reason = "splice runs only on a file that exists")]
    pub(super) fn splice(&mut self, at: usize, old: usize, new: &[String]) {
        let lines = self.lines.as_mut().expect("splice on a missing file");
        lines.splice(at..at + old, new.iter().cloned());
        self.edits.retain(|&(s, _)| s < at || s >= at + old);
        for (s, _) in &mut self.edits {
            if *s >= at + old {
                *s = *s + new.len() - old;
            }
        }
        self.edits.push((at, new.len()));
    }

    /// The text, line ends and permissions a `Move to` or `Copy` hands to its destination. A copy
    /// clones them and leaves this file as it is; a move empties it.
    pub(super) fn take_contents(&mut self, copy: bool) -> File {
        // A move keeps the source's text, so `apply_with` can report what the move removed.
        let (lines, before) = if copy {
            (self.lines.clone(), self.before.clone())
        } else {
            (self.lines.take(), self.before.clone())
        };
        let edits = if copy {
            Vec::new()
        } else {
            std::mem::take(&mut self.edits)
        };
        File {
            lines,
            before,
            crlf: self.crlf,
            trailing_newline: self.trailing_newline,
            perms: self.perms.clone(),
            existed: false,
            dirty: true,
            edits,
        }
    }
}

pub(super) struct Files<'a> {
    pub(super) root: &'a Path,
    pub(super) map: BTreeMap<String, File>,
    pub(super) numbered: Vec<String>,
    pub(super) trailing: Vec<String>,
    pub(super) indented: Vec<String>,
    pub(super) fuzzy_matches: Vec<String>,
    pub(super) fuzzy: bool,
    pub(super) skipped: Vec<usize>,
    pub(super) crlf: Vec<String>,
}

impl Files<'_> {
    pub(super) fn get(&mut self, path: &str) -> Result<&mut File> {
        if !self.map.contains_key(path) {
            let full = self.root.join(path);
            let f = match fs::read(&full) {
                Ok(bytes) => {
                    let text = String::from_utf8(bytes)
                        .map_err(|_| EditError::NotUtf8 { path: path.into() })?;
                    let crlf = text.contains("\r\n");
                    let text = if crlf {
                        text.replace("\r\n", "\n")
                    } else {
                        text
                    };
                    let before = text.clone();
                    let trailing_newline = text.ends_with('\n');
                    let body = text.strip_suffix('\n').unwrap_or(&text);
                    File {
                        before: Some(before),
                        lines: Some(if text.is_empty() {
                            vec![]
                        } else {
                            body.split('\n').map(Into::into).collect()
                        }),
                        crlf,
                        trailing_newline,
                        perms: fs::metadata(&full).ok().map(|m| m.permissions()),
                        existed: true,
                        dirty: false,
                        edits: Vec::new(),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => File {
                    lines: None,
                    before: None,
                    crlf: false,
                    trailing_newline: true,
                    perms: None,
                    existed: false,
                    dirty: false,
                    edits: Vec::new(),
                },
                Err(source) => {
                    return Err(EditError::Read {
                        path: path.into(),
                        source,
                    });
                }
            };
            self.map.insert(path.to_owned(), f);
        }
        self.map
            .get_mut(path)
            .ok_or_else(|| EditError::NotLoaded { path: path.into() })
    }

    pub(super) fn lines(&mut self, path: &str) -> Result<&mut Vec<String>> {
        self.get(path)?
            .lines
            .as_mut()
            .ok_or_else(|| EditError::NotFound { path: path.into() })
    }
}

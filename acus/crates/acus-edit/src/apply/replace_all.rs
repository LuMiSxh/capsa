// SPDX-License-Identifier: MPL-2.0
//! `*** Replace All`: literal or regex replacement across files, directories and globs.

use super::Change;
use super::files::{File, Files};
use crate::error::{EditError, Result};
use crate::parse::Block;
use regex::{Captures, Regex};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Expands the `*** Replace All` paths into files: plain files, directories (walked, respecting
/// `.gitignore`) and globs.
fn expand_paths(root: &Path, specs: &[String]) -> Result<Vec<(String, bool)>> {
    // The CLI passes "" for the working directory, which the walker cannot open.
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let (globs, plain): (Vec<_>, Vec<_>) = specs.iter().partition(|s| s.contains(['*', '?', '[']));
    let mut out = Vec::new();
    let mut dirs = Vec::new();
    for p in plain {
        if root.join(p).is_dir() {
            dirs.push(root.join(p));
        } else {
            out.push((p.clone(), true));
        }
    }
    // Directories are taken whole; globs select files anywhere below the root.
    let mut walks = Vec::new();
    if !dirs.is_empty() {
        walks.push((dirs, Vec::new()));
    }
    if !globs.is_empty() {
        walks.push((
            vec![root.to_path_buf()],
            globs.into_iter().cloned().collect(),
        ));
    }
    let mut found = Vec::new();
    for (roots, include) in walks {
        let hits = Mutex::new(Vec::new());
        let opts = acus_walk::WalkOpts {
            roots,
            include,
            ..Default::default()
        };
        acus_walk::walk(&opts, |p| {
            let rel = p.strip_prefix(root).map(PathBuf::from).unwrap_or(p.into());
            hits.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(acus_walk::display_path(&rel));
        })?;
        found.extend(
            hits.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
    found.sort();
    found.dedup();
    found.retain(|p| !out.iter().any(|(o, _)| o == p));
    out.extend(found.into_iter().map(|p| (p, false)));
    Ok(out)
}

pub(super) fn replace_all(
    files: &mut Files,
    specs: &[String],
    blocks: &[Block],
) -> Result<Vec<Change>> {
    let mut res = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let (search, replace) = (b.search.join("\n"), b.replace.join("\n"));
        let pat = if b.regex {
            format!("(?m){search}")
        } else {
            regex::escape(&search)
        };
        let re = Regex::new(&pat).map_err(|error| EditError::BadRegex {
            block: i + 1,
            error,
        })?;
        res.push((re, replace, b.regex));
    }
    let mut hits = vec![0; blocks.len()];
    let mut changes = Vec::new();
    for (path, explicit) in expand_paths(files.root, specs)? {
        let lines = match files.lines(&path) {
            Ok(l) => l.clone(),
            Err(e) if explicit => return Err(e),
            // Binary or unreadable files met while walking a directory.
            Err(_) => continue,
        };
        let (mut lines, before) = (lines.clone(), lines.len());
        let mut hunks = Vec::new();
        for (k, (re, rep, expand)) in res.iter().enumerate() {
            let h = replace_in(&lines, re, rep, *expand);
            hits[k] += h.len();
            for (at, old, new) in h.into_iter().rev() {
                lines.splice(at..at + old, new.iter().cloned());
                hunks.push((at, old, new));
            }
        }
        if hunks.is_empty() {
            continue;
        }
        // Replay the hunks through `splice` so the printed regions are right.
        let f = files.get(&path)?;
        let marked = |f: &File| f.edits.iter().map(|e| e.1).sum::<usize>();
        let m0 = marked(f);
        for (at, old, new) in hunks {
            f.splice(at, old, &new);
        }
        f.dirty = true;
        // Lines touched by several blocks count once.
        let added = marked(f).saturating_sub(m0);
        changes.push(Change::Modified {
            path,
            added,
            removed: (added + before).saturating_sub(lines.len()),
        });
    }
    if let Some(k) = hits.iter().position(|&n| n == 0) {
        let first = blocks[k].search.iter().find(|l| !l.trim().is_empty());
        return Err(EditError::ReplaceNoMatch {
            block: k + 1,
            search: first.map_or("", |s| s.trim()).into(),
            paths: specs.join(" "),
            regex: blocks[k].regex,
        });
    }
    Ok(changes)
}

/// Replaces every match of `re` and returns `(line, old count, new lines)` per run of touched
/// lines.
fn replace_in(
    lines: &[String],
    re: &Regex,
    rep: &str,
    expand: bool,
) -> Vec<(usize, usize, Vec<String>)> {
    let text = lines.join("\n");
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line_of = |b: usize| starts.partition_point(|&s| s <= b) - 1;
    let end_of = |l: usize| starts.get(l + 1).map_or(text.len(), |&s| s - 1);
    let mut hunks: Vec<(usize, usize, String)> = Vec::new();
    let mut pos = 0;
    for c in re.captures_iter(&text) {
        let Some(m) = c.get(0) else {
            continue;
        };
        if m.is_empty() {
            continue;
        }
        let (l0, l1) = (line_of(m.start()), line_of(m.end()));
        let r = sub(&c, rep, expand);
        match hunks.last_mut() {
            Some((_, e, s)) if *e >= l0 => {
                s.push_str(&text[pos..m.start()]);
                s.push_str(&r);
                *e = (*e).max(l1);
            }
            last => {
                if let Some((_, e, s)) = last {
                    s.push_str(&text[pos..end_of(*e)]);
                }
                hunks.push((l0, l1, format!("{}{r}", &text[starts[l0]..m.start()])));
            }
        }
        pos = m.end();
    }
    if let Some((_, e, s)) = hunks.last_mut() {
        s.push_str(&text[pos..end_of(*e)]);
    }
    hunks
        .into_iter()
        .map(|(a, b, s)| {
            (
                a,
                b - a + 1,
                s.split('\n').map(Into::into).collect::<Vec<String>>(),
            )
        })
        .filter(|(a, n, new)| lines[*a..*a + n] != new[..])
        .collect()
}

fn sub(c: &Captures, rep: &str, expand: bool) -> String {
    let mut r = String::new();
    if expand {
        c.expand(rep, &mut r);
    } else {
        r.push_str(rep);
    }
    r
}

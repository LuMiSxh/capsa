// SPDX-License-Identifier: MPL-2.0
//! The SessionStart line that names the repo's saved scripts. It reads files only: no engine runs
//! on the hook path.

use super::store::{Scope, Store};
use super::usage;
use crate::config::Config;
use std::path::Path;

/// Seconds without a use or a save after which a script drops out of the line.
const STALE_SECS: u64 = 30 * 86_400;
const MAX_NAMES: usize = 5;

/// Returns `acus scripts here: a, b, c` for the repo scripts around `cwd`, most used under `cwd`
/// first, or `None` when the repo has none or none was used or saved within 30 days.
///
/// Costs a `stat` per directory level, to the repository root, when `.acus/scripts` is missing.
pub(crate) fn scripts_here(cfg: &Config, cwd: &Path, now: u64) -> Option<String> {
    let root = cwd.ancestors().find_map(|dir| {
        if dir.join(".acus/scripts").is_dir() {
            return Some(Some(dir));
        }
        dir.join(".git").exists().then_some(None)
    })??;
    let store = Store::new(cfg, Some(root));
    // Logged cwds come from `current_dir`, which resolves symlinks.
    let root = &root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let cwd = &cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let runs = usage::runs(cfg);
    let mut ranked: Vec<(usize, usize, String)> = Vec::new();
    for name in store.names(Scope::Repo) {
        let meta = store.meta(Scope::Repo, &name);
        let saved = meta.and_then(|m| m.latest().map(|v| v.at)).unwrap_or(0);
        let mine: Vec<_> = runs
            .iter()
            .filter(|u| u.run.name == name && u.run.scope == Scope::Repo)
            .filter(|u| usage::under(&u.run, root))
            .collect();
        let last = mine.iter().map(|u| u.ts).max().unwrap_or(0).max(saved);
        if now.saturating_sub(last) > STALE_SECS {
            continue;
        }
        let here = mine.iter().filter(|u| usage::under(&u.run, cwd)).count();
        ranked.push((here, mine.len(), name));
    }
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    let names: Vec<_> = ranked.into_iter().take(MAX_NAMES).map(|r| r.2).collect();
    (!names.is_empty()).then(|| format!("acus scripts here: {}", names.join(", ")))
}

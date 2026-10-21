// SPDX-License-Identifier: MPL-2.0
//! Owns the directories a lookup searches: `PATH` first, then the user bin
//! directories that tools commonly install into.

use std::path::{Path, PathBuf};

use super::{Env, Fs, Platform};

/// Returns the directories after `PATH` where user-installed tools commonly live. Only the
/// home-relative ones need `env.home`; nvm's node directories are listed by `fs`.
pub fn extra_dirs(env: &Env, fs: &dyn Fs) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut add = |p: Option<PathBuf>| dirs.extend(p);
    let home = |rel: &str| env.home.as_ref().map(|h| h.join(rel));
    if env.platform == Platform::Windows {
        add(env.appdata.as_ref().map(|a| a.join("npm")));
        add(env.local_appdata.as_ref().map(|a| a.join("pnpm")));
        add(env.local_appdata.as_ref().map(|a| a.join("pnpm/bin")));
        add(home(".bun/bin"));
        add(env.local_appdata.as_ref().map(|a| a.join("Volta/bin")));
        add(home(".cargo/bin"));
        add(home(".local/bin"));
    } else {
        add(home(".local/bin"));
        add(home(".cargo/bin"));
        add(home("Library/pnpm"));
        add(home("Library/pnpm/bin"));
        add(home(".local/share/pnpm"));
        add(home(".local/share/pnpm/bin"));
        add(home(".bun/bin"));
        add(Some(PathBuf::from("/opt/homebrew/bin")));
        add(Some(PathBuf::from("/usr/local/bin")));
        add(home(".volta/bin"));
        add(home(".local/share/fnm/aliases/default/bin"));
        add(home("Library/Application Support/fnm/aliases/default/bin"));
        add(home(".fnm/aliases/default/bin"));
    }
    // nvm keeps one directory per installed node; newest first.
    if let Some(versions) = home(".nvm/versions/node") {
        let mut found = fs.subdirs(&versions);
        found.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
        dirs.extend(found.into_iter().map(|p| p.join("bin")));
    }
    dirs
}

/// Parses `v20.11.1` into `[20, 11, 1]`, so node versions order numerically.
fn version_key(dir: &Path) -> Vec<u64> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    name.trim_start_matches('v')
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// Returns the `PATH` entries first, then [`extra_dirs`], without empty entries or duplicates.
pub fn search_dirs(env: &Env, fs: &dyn Fs) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in env.path.iter().cloned().chain(extra_dirs(env, fs)) {
        if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Returns the `PATH` for a child that runs `program`: the program's directory first
/// (so a `#!/usr/bin/env node` shim finds its sibling `node`), then the
/// inherited entries, then the user bin directories that exist.
pub fn child_path(env: &Env, fs: &dyn Fs, program: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |d: PathBuf| {
        if !d.as_os_str().is_empty() && !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    if let Some(parent) = program.parent() {
        push(parent.to_path_buf());
    }
    env.path.iter().cloned().for_each(&mut push);
    for dir in extra_dirs(env, fs) {
        if fs.is_dir(&dir) {
            push(dir);
        }
    }
    dirs
}

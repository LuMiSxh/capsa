// SPDX-License-Identifier: MPL-2.0
//! Owns parallel file discovery, honouring ignore files and include/exclude globs.

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::{WalkBuilder, WalkState};
use std::path::{Path, PathBuf};

/// Why a walk could not start.
#[derive(Debug, thiserror::Error)]
pub enum WalkError {
    /// Every root is missing; `path` is the first one, as printed.
    #[error("path not found: {path}")]
    PathNotFound { path: String },
    /// A glob does not compile.
    #[error("invalid glob `{glob}`")]
    InvalidGlob {
        glob: String,
        #[source]
        source: globset::Error,
    },
    /// The globs do not form a set.
    #[error(transparent)]
    GlobSet(#[from] globset::Error),
}

/// Configures `walk`. The default visits `.`, honours ignore files and skips hidden entries.
#[derive(Debug, Clone, Default)]
pub struct WalkOpts {
    /// Files or directories to visit; empty means `.`.
    pub roots: Vec<PathBuf>,
    /// Keep only paths matching one of these globs (empty keeps all).
    pub include: Vec<String>,
    /// Drop paths matching one of these globs.
    pub exclude: Vec<String>,
    /// Visit hidden files and directories too.
    pub hidden: bool,
    /// Also visit files excluded by .gitignore/.ignore.
    pub no_ignore: bool,
    /// Follow symbolic links to files and directories, as rg -L (not grep -L).
    pub follow: bool,
}

fn glob_set(globs: &[String]) -> Result<Option<GlobSet>, WalkError> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for g in globs {
        // As in rg and gitignore: a glob without a `/` matches a file name at any depth, one with
        // a `/` is relative to the search root.
        let pat = if g.contains('/') {
            g.clone()
        } else {
            format!("**/{g}")
        };
        b.add(Glob::new(&pat).map_err(|source| WalkError::InvalidGlob {
            glob: g.clone(),
            source,
        })?);
    }
    Ok(Some(b.build()?))
}

/// Calls `visit` once per matching file, concurrently from several threads.
pub fn walk<F: Fn(&Path) + Sync>(opts: &WalkOpts, visit: F) -> Result<(), WalkError> {
    let include = glob_set(&opts.include)?;
    let exclude = glob_set(&opts.exclude)?;
    let roots = if opts.roots.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.roots.clone()
    };
    // Missing roots are reported and skipped while at least one root exists.
    let (roots, missing): (Vec<PathBuf>, Vec<PathBuf>) =
        roots.into_iter().partition(|r| r.exists());
    if roots.is_empty() {
        return Err(WalkError::PathNotFound {
            path: display_path(&missing[0]),
        });
    }
    for r in &missing {
        eprintln!("warning: path not found: {}", display_path(r));
    }
    let keep = {
        let roots = roots.clone();
        move |p: &Path| include.as_ref().is_none_or(|s| s.is_match(rel(&roots, p)))
    };
    let mut b = WalkBuilder::new(&roots[0]);
    for r in &roots[1..] {
        b.add(r);
    }
    b.standard_filters(!opts.no_ignore)
        .hidden(!opts.hidden)
        .follow_links(opts.follow)
        .require_git(false);
    if let Some(ex) = exclude {
        // Like rg: `!dist` drops any file or directory named `dist`, `!src/gen` that path;
        // excluded directories are not descended into.
        let roots = roots.clone();
        b.filter_entry(move |e| {
            let r = rel(&roots, e.path());
            !(ex.is_match(r) || r.file_name().is_some_and(|n| ex.is_match(n)))
        });
    }
    b.build_parallel().run(|| {
        let (keep, visit) = (&keep, &visit);
        Box::new(move |entry| {
            // ponytail: unreadable entries are skipped silently. Count and report them if this
            // hides a problem from users.
            if let Ok(e) = entry
                && e.file_type().is_some_and(|t| t.is_file())
                && keep(e.path())
            {
                visit(e.path());
            }
            WalkState::Continue
        })
    });
    Ok(())
}

/// Globs see the path relative to its root; a file given as root is matched as-is.
fn rel<'a>(roots: &[PathBuf], p: &'a Path) -> &'a Path {
    roots
        .iter()
        .find_map(|r| p.strip_prefix(r).ok())
        .filter(|r| !r.as_os_str().is_empty())
        .unwrap_or(p)
}

/// Formats a path the way every acus output prints it: `/`-separated, without a leading `./`.
pub fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("./").map(str::to_owned).unwrap_or(s)
}

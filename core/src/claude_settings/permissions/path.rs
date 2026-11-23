// SPDX-License-Identifier: MPL-2.0
//! `Read(...)` and `Edit(...)` rules: gitignore-style path patterns, as in Claude Code.
//!
//! Prefixes: `//path` is absolute, `~/path` is under the home directory, `/path` is under the
//! project root, `./path` and a bare `path` are under the current directory. The rest is a
//! gitignore pattern (`*`, `**`, a trailing `/`) matched against the path and its parents.
//! Approximations: a bare pattern without a slash (`.env`) matches at any depth below the
//! current directory, as gitignore does, where Claude Code anchors it; `!` negation inside one
//! rule only cancels that rule.
//!
//! Paths are compared as `/`-separated strings, lower-cased on Windows, with a drive letter
//! written `/c/...`, so `//c/Users/me/**` names a Windows path the way Claude Code writes it.

use ignore::gitignore::GitignoreBuilder;
use std::path::{Component, Path, PathBuf};

/// The directories a rule's prefix is relative to, as comparison strings.
pub(super) struct Place {
    root: String,
    cwd: String,
    home: Option<String>,
}

impl Place {
    pub(super) fn new(root: &Path, cwd: &Path, home: Option<&Path>) -> Self {
        let canonical = |p: &Path| virt(&p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
        Place {
            root: canonical(root),
            cwd: canonical(cwd),
            home: home.map(canonical),
        }
    }

    /// Returns whether the comparison string `target` is the working directory or below it.
    pub(super) fn in_cwd(&self, target: &str) -> bool {
        Path::new(target).starts_with(&self.cwd)
    }
}

/// Returns the strings a rule is matched against: the path made absolute and tidied of `.` and
/// `..`, and, when it exists or its parent does, the path with symlinks resolved. A rule that
/// names either one applies.
pub(super) fn targets(path: &Path, cwd: &Path) -> Vec<String> {
    // Start from the canonical working directory, as `Place` does, so a short or aliased
    // cwd (`C:\Users\RUNNER~1\...`) yields the same strings the rule prefixes use.
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut lexical = PathBuf::new();
    for part in cwd.join(path).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other),
        }
    }
    let mut found = vec![virt(&lexical)];
    let real = lexical.canonicalize().ok().or_else(|| {
        let parent = lexical.parent()?.canonicalize().ok()?;
        Some(parent.join(lexical.file_name()?))
    });
    if let Some(real) = real {
        let real = virt(&real);
        if !found.contains(&real) {
            found.push(real);
        }
    }
    found
}

/// Returns whether the comparison string `target` is `.git` or below a directory of that name.
pub(super) fn in_git_dir(target: &str) -> bool {
    target
        .split('/')
        .any(|part| part.eq_ignore_ascii_case(".git"))
}

/// Returns whether the rule's `spec` covers the path `target`.
pub(super) fn matches(spec: &str, target: &str, place: &Place) -> bool {
    let (base, pattern) = if spec.starts_with("//") {
        ("/", spec[1..].to_owned())
    } else if let Some(rest) = spec.strip_prefix("~/") {
        let Some(home) = &place.home else {
            return false;
        };
        (home.as_str(), format!("/{rest}"))
    } else if spec.starts_with('/') {
        (place.root.as_str(), spec.to_owned())
    } else if let Some(rest) = spec.strip_prefix("./") {
        (place.cwd.as_str(), format!("/{rest}"))
    } else {
        (place.cwd.as_str(), spec.to_owned())
    };
    let Ok(relative) = Path::new(target).strip_prefix(base) else {
        return false;
    };
    if relative.as_os_str().is_empty() {
        return false;
    }
    let mut builder = GitignoreBuilder::new(base);
    builder.case_insensitive(cfg!(windows)).ok();
    if builder.add_line(None, &pattern).is_err() {
        return false;
    }
    // The path is made relative first: the matcher panics on a path it cannot place under its root.
    builder.build().is_ok_and(|rules| {
        rules
            .matched_path_or_any_parents(relative, false)
            .is_ignore()
    })
}

/// Returns `path` as a `/`-separated string with a drive letter as a first segment, lower-cased
/// on Windows.
fn virt(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.trim_prefix("//?/");
    let mut out = match text.as_bytes() {
        [drive, b':', ..] if drive.is_ascii_alphabetic() => {
            format!("/{}{}", drive.to_ascii_lowercase() as char, &text[2..])
        }
        _ if text.starts_with('/') => text.to_owned(),
        _ => format!("/{text}"),
    };
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    if cfg!(windows) {
        out.make_ascii_lowercase();
    }
    out
}

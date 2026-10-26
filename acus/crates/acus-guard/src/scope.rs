// SPDX-License-Identifier: MPL-2.0
//! Owns which paths count as inside the session's repository.

use super::*;

impl Scope {
    pub fn new(cwd: Option<&str>) -> Option<Scope> {
        let cwd = PathBuf::from(cwd?);
        let root = cwd
            .ancestors()
            .find(|d| d.join(".git").exists())
            .unwrap_or(&cwd)
            .to_path_buf();
        let canon = |p: PathBuf| p.canonicalize().unwrap_or(p);
        Some(Scope {
            cwd: canon(cwd),
            root: canon(root),
            fixed: false,
        })
    }

    /// Builds a scope that never touches the filesystem: `root` is taken as given and `~` is a
    /// fixed home, so results do not depend on the machine that runs them.
    #[cfg(test)]
    pub(super) fn fixed(root: &str) -> Scope {
        Scope {
            cwd: PathBuf::from(root),
            root: PathBuf::from(root),
            fixed: true,
        }
    }

    /// Answers for a fixed scope: a path under the root exists when it has a file extension.
    fn exists(&self, p: &Path) -> bool {
        if self.fixed {
            p.starts_with(&self.root) && p.extension().is_some()
        } else {
            p.exists()
        }
    }

    /// Resolves symlinks only on a real filesystem.
    fn real(&self, p: PathBuf) -> PathBuf {
        if self.fixed {
            p
        } else {
            p.canonicalize().unwrap_or(p)
        }
    }

    /// Reports whether `path` (absolute, relative to the session, `~/…` or `$HOME/…`) is in the
    /// repository. A path that cannot be resolved (other variables) counts as inside.
    pub(super) fn inside(&self, path: &str) -> bool {
        let p = unquote(path);
        // A fixed scope must not depend on the machine's home directory.
        let home = || {
            if self.fixed {
                home_dir(Some(self)).map(std::ffi::OsString::from)
            } else {
                std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
            }
        };
        let rest = p
            .strip_prefix('~')
            .or_else(|| p.strip_prefix("$HOME"))
            .or_else(|| p.strip_prefix("${HOME}"))
            .filter(|r| r.is_empty() || r.starts_with(['/', '\\']));
        let path = match rest {
            Some(r) => match home() {
                Some(h) => PathBuf::from(h).join(r.trim_start_matches(['/', '\\'])),
                None => return true,
            },
            None if p.starts_with('$') => return true,
            None => PathBuf::from(p),
        };
        let mut full = PathBuf::new();
        for c in self.cwd.join(path).components() {
            match c {
                std::path::Component::ParentDir => {
                    full.pop();
                }
                std::path::Component::CurDir => {}
                c => full.push(c),
            }
        }
        // Resolve symlinks of the part that exists (`/tmp` is `/private/tmp` on macOS).
        let mut base = full.clone();
        let mut tail = PathBuf::new();
        while !self.exists(&base) {
            let Some(name) = base.file_name().map(PathBuf::from) else {
                break;
            };
            tail = name.join(&tail);
            base.pop();
        }
        self.real(base).join(tail).starts_with(&self.root)
    }

    /// Reports whether `path` names an existing file (a fixed scope takes one under the root with
    /// an extension). Returns the path of a file under the scope, or `None` where the scope
    /// does not read the disk.
    pub(super) fn path(&self, path: &str) -> Option<PathBuf> {
        (!self.fixed).then(|| self.cwd.join(unquote(path)))
    }

    /// Answers from the disk: the path is an existing directory. A missing path is no answer,
    /// since the scope does not follow a `cd` earlier in the command; a fixed scope cannot
    /// read the disk.
    pub(super) fn is_dir(&self, path: &str) -> bool {
        !self.fixed && self.cwd.join(unquote(path)).is_dir()
    }

    pub(super) fn is_file(&self, path: &str) -> bool {
        let p = self.cwd.join(unquote(path));
        if self.fixed {
            p.starts_with(&self.root) && p.extension().is_some()
        } else {
            p.is_file()
        }
    }
}

/// Without a known session directory every path counts as a file.
pub(super) fn is_file(scope: Option<&Scope>, path: &str) -> bool {
    scope.is_none_or(|s| s.is_file(path))
}

pub(super) fn is_dir(scope: Option<&Scope>, path: &str) -> bool {
    scope.is_some_and(|s| s.is_dir(path))
}

/// Returns the home directory `~` stands for: a fixed scope has its own, so results do not
/// depend on the machine that runs them.
pub(super) fn home_dir(scope: Option<&Scope>) -> Option<String> {
    match scope {
        Some(s) if s.fixed => Some("/home/user".to_owned()),
        // A Windows home has `\`, which an unquoted word cannot hold; output paths use `/`.
        _ => std::env::var("HOME").ok().map(|h| {
            if cfg!(windows) {
                h.replace('\x5c', "/")
            } else {
                h
            }
        }),
    }
}

/// Without a known session directory every path counts as inside.
pub(super) fn inside(scope: Option<&Scope>, path: &str) -> bool {
    scope.is_none_or(|s| s.inside(path))
}

/// Returns `path` relative to the session's directory when inside it, else as given.
pub(super) fn relative(path: &str, cwd: Option<&str>) -> String {
    match cwd.and_then(|c| Path::new(path).strip_prefix(c).ok()) {
        Some(p) if p.as_os_str().is_empty() => ".".to_owned(),
        Some(p) => p.display().to_string(),
        None => path.to_owned(),
    }
}

/// Returns the session's git repository, or its directory when it is in none. Reads of files
/// outside it (`~/.claude`, `/tmp`, `~/.config`) are on purpose and are left alone.
pub struct Scope {
    cwd: PathBuf,
    root: PathBuf,
    fixed: bool,
}

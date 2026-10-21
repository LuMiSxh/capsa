// SPDX-License-Identifier: MPL-2.0
//! Owns program lookup on the PATH and the user bin directories, which a tool started
//! from a GUI often lacks (Zed does not inherit the shell's `PATH`).

mod dirs;
#[cfg(test)]
mod tests;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub use dirs::{child_path, extra_dirs, search_dirs};

const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// The operating system, for the lookup rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    /// The platform this binary runs on.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }

    /// Returns the name shown in reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }
}

/// Holds the parts of the environment that the lookup depends on.
#[derive(Clone, Debug)]
pub struct Env {
    pub platform: Platform,
    /// The entries of `PATH`, in order.
    pub path: Vec<PathBuf>,
    /// `HOME`, or `USERPROFILE` on Windows. Without it no user directory is searched.
    pub home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    /// Lowercase executable extensions with the dot (`PATHEXT`).
    pub pathext: Vec<String>,
}

impl Env {
    /// Reads the lookup inputs from the process environment.
    pub fn from_process() -> Self {
        let platform = Platform::current();
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let home_var = if platform == Platform::Windows {
            "USERPROFILE"
        } else {
            "HOME"
        };
        let pathext = var("PATHEXT")
            .map(|v| parse_pathext(&v.to_string_lossy()))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| parse_pathext(DEFAULT_PATHEXT));
        Self {
            platform,
            path: var("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
            home: var(home_var).map(PathBuf::from),
            appdata: var("APPDATA").map(PathBuf::from),
            local_appdata: var("LOCALAPPDATA").map(PathBuf::from),
            pathext,
        }
    }

    /// Builds an `Env` with only the entries of `path`: no home directory, so no user
    /// directory. For callers that are handed their own PATH.
    pub fn from_path(path: &OsStr) -> Self {
        Self {
            platform: Platform::current(),
            path: std::env::split_paths(path).collect(),
            home: None,
            appdata: None,
            local_appdata: None,
            pathext: parse_pathext(DEFAULT_PATHEXT),
        }
    }
}

/// Answers the file system questions that the lookup asks.
pub trait Fs {
    /// Returns whether `path` is an existing file the user can run.
    fn is_executable(&self, path: &Path) -> bool;
    /// Returns whether `path` is a directory.
    fn is_dir(&self, path: &Path) -> bool;
    /// Returns the sub-directories of `path`, in no particular order.
    fn subdirs(&self, path: &Path) -> Vec<PathBuf>;
}

/// Answers from the real file system.
pub struct RealFs;

impl Fs for RealFs {
    fn is_executable(&self, path: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        if !meta.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn subdirs(&self, path: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(path)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Parses a `PATHEXT` value into its extensions: lowercase, with their dot.
pub fn parse_pathext(text: &str) -> Vec<String> {
    text.split(';')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| e.starts_with('.') && e.len() > 1)
        .collect()
}

/// Returns the first runnable `name` in [`search_dirs`], skipping the directory `exclude`.
pub fn find_in(env: &Env, fs: &dyn Fs, name: &str, exclude: Option<&Path>) -> Option<PathBuf> {
    let names = candidate_names(env, name);
    search_dirs(env, fs)
        .into_iter()
        .filter(|dir| exclude != Some(dir.as_path()))
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| fs.is_executable(p))
}

/// Finds `name` like [`find_in`], over the process environment and the real file system.
pub fn find(name: &str) -> Option<PathBuf> {
    find_in(&Env::from_process(), &RealFs, name, None)
}

/// Finds `name` in the PATH value `path` only, skipping the directory `exclude`.
/// Returns the file and the index of its directory among the PATH entries.
///
/// Empty and relative entries are skipped: they mean the current directory,
/// which a sandboxed agent can fill, so the result is always an absolute path.
pub fn find_on_path(name: &str, path: &OsStr, exclude: Option<&Path>) -> Option<(usize, PathBuf)> {
    let env = Env::from_path(path);
    let names = candidate_names(&env, name);
    env.path
        .iter()
        .enumerate()
        .filter(|(_, dir)| dir.is_absolute())
        .filter(|(_, dir)| exclude != Some(dir.as_path()))
        .flat_map(|(index, dir)| names.iter().map(move |n| (index, dir.join(n))))
        .find(|(_, file)| RealFs.is_executable(file))
}

/// Returns the index of `dir` among the entries of the PATH value `path`.
pub fn path_index(path: &OsStr, dir: &Path) -> Option<usize> {
    std::env::split_paths(path).position(|entry| entry == dir)
}

/// Returns the file names that can be `name` on this platform: `name` itself on Unix; on
/// Windows `name` plus each `PATHEXT` extension (a bare `npx` is a shell script
/// there and cannot be run), unless `name` already has one of them.
fn candidate_names(env: &Env, name: &str) -> Vec<String> {
    if env.platform != Platform::Windows {
        return vec![name.to_owned()];
    }
    let lower = name.to_ascii_lowercase();
    if env.pathext.iter().any(|e| lower.ends_with(e.as_str())) {
        return vec![name.to_owned()];
    }
    env.pathext.iter().map(|e| format!("{name}{e}")).collect()
}

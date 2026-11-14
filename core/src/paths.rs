// SPDX-License-Identifier: MPL-2.0
//! Owns `~` expansion for path arguments, since `acus run` lines and patch text get no shell, and
//! the per-tool config and state directories.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Returns the directory of a tool's configuration files.
///
/// `~/Library/Application Support/<tool>` on macOS, `%APPDATA%\<tool>` on Windows and
/// `$XDG_CONFIG_HOME/<tool>` or `~/.config/<tool>` elsewhere. Nothing is created.
pub fn config_dir(tool: &str) -> PathBuf {
    config_dir_with(tool, std::env::home_dir(), |k| std::env::var_os(k))
}

/// Returns the directory of a tool's state files (ledgers, logs, snapshots).
///
/// `$XDG_STATE_HOME/<tool>` or `~/.local/state/<tool>` on macOS and Linux,
/// `%LOCALAPPDATA%\<tool>\state` on Windows. Nothing is created.
pub fn state_dir(tool: &str) -> PathBuf {
    state_dir_with(tool, std::env::home_dir(), |k| std::env::var_os(k))
}

/// Returns the file `name` in a tool's state directory, where every tool keeps its logs.
///
/// That is `<state_dir(tool)>/<name>`. Returns `None` when neither a home directory nor the state
/// root variable is known, since the directory would then be relative to the working directory.
/// Nothing is created.
pub fn log_file(tool: &str, name: &str) -> Option<PathBuf> {
    log_file_with(tool, name, std::env::home_dir(), |k| std::env::var_os(k))
}

fn config_dir_with(
    tool: &str,
    home: Option<PathBuf>,
    env: impl Fn(&str) -> Option<OsString>,
) -> PathBuf {
    let home = home.unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support").join(tool)
    } else if cfg!(windows) {
        env("APPDATA").map_or(home, PathBuf::from).join(tool)
    } else {
        env("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), PathBuf::from)
            .join(tool)
    }
}

fn state_dir_with(
    tool: &str,
    home: Option<PathBuf>,
    env: impl Fn(&str) -> Option<OsString>,
) -> PathBuf {
    let home = home.unwrap_or_default();
    if cfg!(windows) {
        env("LOCALAPPDATA")
            .map_or(home, PathBuf::from)
            .join(tool)
            .join("state")
    } else {
        env("XDG_STATE_HOME")
            .map_or_else(|| home.join(".local/state"), PathBuf::from)
            .join(tool)
    }
}

/// The variable that names the state root: `LOCALAPPDATA` on Windows, `XDG_STATE_HOME` elsewhere.
const STATE_ROOT_VAR: &str = if cfg!(windows) {
    "LOCALAPPDATA"
} else {
    "XDG_STATE_HOME"
};

fn log_file_with(
    tool: &str,
    name: &str,
    home: Option<PathBuf>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    let known = home.is_some() || env(STATE_ROOT_VAR).is_some();
    known.then(|| state_dir_with(tool, home, &env).join(name))
}

/// Expands a leading `~` or `~/` (`~\` on Windows) to the home directory.
///
/// Command-line words get a shell, but `acus run` lines and patch text do not, so the tools
/// expand the path themselves. `~user` and a `~` anywhere else stay as written, and so does
/// the path when no home directory is known.
pub fn expand_home(path: &str) -> PathBuf {
    expand_with_home(path, std::env::home_dir().as_deref())
}

fn expand_with_home(path: &str, home: Option<&Path>) -> PathBuf {
    let rest = match path.strip_prefix('~') {
        Some(r) if r.is_empty() || r.starts_with(['/', '\\']) => r,
        _ => return PathBuf::from(path),
    };
    match home {
        Some(home) => {
            let mut expanded = home.as_os_str().to_owned();
            expanded.push(rest);
            PathBuf::from(expanded)
        }
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::{config_dir_with, state_dir_with};
    use super::{expand_with_home, log_file_with};
    #[cfg(unix)]
    use std::ffi::OsString;
    use std::path::Path;
    #[cfg(unix)]
    use std::path::PathBuf;

    const HOME: &str = "/home/user";

    fn expand(path: &str) -> String {
        expand_with_home(path, Some(Path::new(HOME)))
            .display()
            .to_string()
    }

    #[test]
    fn bare_tilde_is_the_home_directory() {
        assert_eq!(expand("~"), HOME);
    }

    #[test]
    fn tilde_slash_joins_the_rest_to_home() {
        assert_eq!(expand("~/src/a.rs"), "/home/user/src/a.rs");
    }

    #[test]
    fn tilde_backslash_joins_the_rest_to_home() {
        assert_eq!(expand("~\\src"), "/home/user\\src");
    }

    #[test]
    fn tilde_user_form_is_kept_as_written() {
        assert_eq!(expand("~bob/src"), "~bob/src");
    }

    #[test]
    fn tilde_inside_a_path_is_kept_as_written() {
        assert_eq!(expand("src/~/a.rs"), "src/~/a.rs");
        assert_eq!(expand("a~"), "a~");
    }

    #[test]
    fn path_is_kept_when_no_home_is_known() {
        assert_eq!(
            expand_with_home("~/src", None).display().to_string(),
            "~/src"
        );
    }

    #[test]
    fn path_without_tilde_is_unchanged() {
        assert_eq!(expand("/abs/x"), "/abs/x");
    }

    #[test]
    #[cfg(unix)]
    fn state_dir_prefers_xdg_state_home() {
        let env = |k: &str| (k == "XDG_STATE_HOME").then(|| OsString::from("/x/state"));
        assert_eq!(
            state_dir_with("census", Some(PathBuf::from(HOME)), env),
            Path::new("/x/state/census")
        );
    }

    #[test]
    #[cfg(unix)]
    fn state_dir_falls_back_to_local_state_in_home() {
        assert_eq!(
            state_dir_with("census", Some(PathBuf::from(HOME)), |_| None),
            Path::new("/home/user/.local/state/census")
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn config_dir_is_application_support_on_macos() {
        assert_eq!(
            config_dir_with("acus", Some(PathBuf::from(HOME)), |_| None),
            Path::new("/home/user/Library/Application Support/acus")
        );
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn config_dir_honours_xdg_config_home() {
        let env = |k: &str| (k == "XDG_CONFIG_HOME").then(|| OsString::from("/x/cfg"));
        assert_eq!(
            config_dir_with("acus", Some(PathBuf::from(HOME)), env),
            Path::new("/x/cfg/acus")
        );
    }

    #[test]
    #[cfg(unix)]
    fn log_file_joins_the_name_to_the_state_dir() {
        assert_eq!(
            log_file_with("acus", "guard.jsonl", Some(PathBuf::from(HOME)), |_| None),
            Some(PathBuf::from("/home/user/.local/state/acus/guard.jsonl"))
        );
    }

    #[test]
    fn log_file_is_none_without_a_home_or_state_root() {
        assert_eq!(log_file_with("acus", "guard.jsonl", None, |_| None), None);
    }

    #[test]
    #[cfg(unix)]
    fn log_file_uses_the_state_root_when_no_home_is_known() {
        let env = |k: &str| (k == "XDG_STATE_HOME").then(|| OsString::from("/x/state"));
        assert_eq!(
            log_file_with("acus", "guard.jsonl", None, env),
            Some(PathBuf::from("/x/state/acus/guard.jsonl"))
        );
    }
}

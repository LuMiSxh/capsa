// SPDX-License-Identifier: MPL-2.0
//! Owns `~` expansion for path arguments, since `acus run` lines and patch text get no shell.

use std::path::Path;

/// Expands a leading `~` or `~/` (`~\` on Windows) to the home directory.
///
/// Command-line words get a shell, but `acus run` lines and patch text do not, so the tools
/// expand the path themselves. `~user` and a `~` anywhere else stay as written, and so does
/// the path when no home directory is known.
pub fn expand_home(path: &str) -> String {
    expand_with_home(path, std::env::home_dir().as_deref())
}

fn expand_with_home(path: &str, home: Option<&Path>) -> String {
    let rest = match path.strip_prefix('~') {
        Some(r) if r.is_empty() || r.starts_with(['/', '\\']) => r,
        _ => return path.to_owned(),
    };
    match home {
        Some(home) => format!("{}{rest}", home.display()),
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::expand_with_home;
    use std::path::Path;

    const HOME: &str = "/home/user";

    fn expand(path: &str) -> String {
        expand_with_home(path, Some(Path::new(HOME)))
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
        assert_eq!(expand_with_home("~/src", None), "~/src");
    }

    #[test]
    fn path_without_tilde_is_unchanged() {
        assert_eq!(expand("/abs/x"), "/abs/x");
    }
}

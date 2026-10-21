// SPDX-License-Identifier: MPL-2.0
//! Owns the judgement of which paths the sandbox can write, from the generated srt settings:
//! velum's trusted files (srt, the real claude, the velum binary) must not be among them.

use crate::policy::PolicyError;
use serde_json::Value;
use std::path::Path;

fn entries<'a>(settings: &'a Value, key: &str) -> impl Iterator<Item = &'a str> {
    settings
        .pointer(&format!("/filesystem/{key}"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|entry| entry.starts_with('/'))
}

/// Returns whether `entry` covers `path`, lexically or after resolving symlinks. A glob entry is
/// judged by its literal prefix, which is what it covers at least.
fn covers(entry: &str, path: &Path) -> bool {
    let literal = Path::new(entry.split(['*', '?', '[']).next().unwrap_or(""));
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (real_entry, real_path) = (real(literal), real(path));
    [path, real_path.as_path()]
        .iter()
        .any(|p| p.starts_with(literal) || p.starts_with(&real_entry))
}

/// Returns the first `allowWrite` entry that covers `path`, ignoring `denyWrite`.
pub fn allowed_by(settings: &Value, path: &Path) -> Option<String> {
    entries(settings, "allowWrite")
        .find(|entry| covers(entry, path))
        .map(str::to_string)
}

/// Returns the first `allowWrite` entry that covers `path` when no `denyWrite` entry does. A glob
/// deny entry is not counted: its literal prefix covers more than the glob does.
pub fn writable_by(settings: &Value, path: &Path) -> Option<String> {
    let entry = allowed_by(settings, path)?;
    let denied = entries(settings, "denyWrite")
        .filter(|deny| !deny.contains(['*', '?', '[']))
        .any(|deny| covers(deny, path));
    (!denied).then_some(entry)
}

/// Fails when the sandbox could write the trusted file `path` (`what` names it).
pub fn ensure_not_writable(
    settings: &Value,
    what: &'static str,
    path: &Path,
) -> Result<(), PolicyError> {
    match writable_by(settings, path) {
        None => Ok(()),
        Some(entry) => Err(PolicyError::TrustedWritable {
            what,
            path: path.to_path_buf(),
            entry,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Policy;
    use crate::srt::settings;

    #[test]
    fn srt_in_the_pnpm_bin_dir_is_not_writable_with_the_defaults() {
        let policy = Policy::builtin(Path::new("/work/proj"));
        let value = settings(&policy, "/Users/a").unwrap();
        for srt in [
            "/Users/a/Library/pnpm/bin/srt",
            "/Users/a/.local/bin/srt",
            "/opt/homebrew/bin/srt",
        ] {
            let path = Path::new(srt);
            assert_eq!(allowed_by(&value, path), None, "{srt}");
            assert!(ensure_not_writable(&value, "srt", path).is_ok(), "{srt}");
        }
    }

    #[test]
    fn paths_under_cwd_or_a_write_entry_are_writable_unless_denied() {
        let policy = Policy::builtin(Path::new("/work/proj"));
        let value = settings(&policy, "/Users/a").unwrap();
        let writable = |p: &str| writable_by(&value, Path::new(p));
        assert_eq!(
            writable("/work/proj/bin/velum").as_deref(),
            Some("/work/proj")
        );
        assert!(writable("/Users/a/.claude/other/claude").is_some());
        assert_eq!(writable("/Users/a/.claude/local/claude"), None);
        assert!(allowed_by(&value, Path::new("/Users/a/.claude/local/claude")).is_some());
        assert_eq!(writable("/Users/a/.cargo/bin/velum"), None);
        assert!(matches!(
            ensure_not_writable(&value, "velum", Path::new("/work/proj/velum")),
            Err(PolicyError::TrustedWritable { .. })
        ));
    }
}

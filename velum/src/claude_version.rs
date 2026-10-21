// SPDX-License-Identifier: MPL-2.0
//! Reads the version of the real claude and judges it against the supported floor.

use anyhow::Result;
use capsa_core::hint::Hinted;
use capsa_core::which;
use std::ffi::OsStr;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The oldest Claude Code that fixes CVE-2026-55607 (GHSA-7835-87q9-rgvv): a worktree named
/// `.git` with symlinks and git's fsmonitor let a repository write `~/.zshenv` and leave
/// Seatbelt.
pub const FLOOR: Version = Version(2, 1, 163);

/// How long `claude --version` may take before it counts as unreadable.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// A `major.minor.patch` version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// How an installed claude compares with `FLOOR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Supported(Version),
    Below(Version),
    Unreadable,
}

/// Finds the first version number (`2.1.163`, with an optional `v`) in the output of
/// `claude --version`, such as `2.1.163 (Claude Code)`.
pub fn parse(text: &str) -> Option<Version> {
    text.split_whitespace().find_map(|word| {
        let mut parts = word.trim_start_matches('v').splitn(3, '.');
        let number = |part: Option<&str>| {
            let digits: String = part?.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        };
        Some(Version(
            number(parts.next())?,
            number(parts.next())?,
            number(parts.next())?,
        ))
    })
}

/// Runs `bin --version` with a minimal environment, outside the sandbox, and compares the
/// result with `FLOOR`.
pub fn verdict(bin: &Path, path: &OsStr, home: &str) -> Verdict {
    match read(bin, path, home) {
        Some(version) if version >= FLOOR => Verdict::Supported(version),
        Some(version) => Verdict::Below(version),
        None => Verdict::Unreadable,
    }
}

fn read(bin: &Path, path: &OsStr, home: &str) -> Option<Version> {
    let mut child = Command::new(bin)
        .arg("--version")
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    parse(&text)
}

/// The hint that follows a version below `FLOOR`.
pub fn update_hint() -> String {
    format!("update Claude Code to {FLOOR} or newer (`claude update`)")
}

/// The claude that `cmd` would start, when its program is named `claude`: the path itself, or
/// the first match on `path`.
pub fn claude_in(cmd: &[String], path: &OsStr) -> Option<PathBuf> {
    let program = Path::new(cmd.first()?);
    if program.file_name()? != "claude" {
        return None;
    }
    if program.components().count() > 1 {
        return Some(program.to_path_buf());
    }
    which::find_on_path("claude", path, None).map(|(_, found)| found)
}

/// Refuses to start a claude below `FLOOR` or whose version cannot be read.
/// Does nothing when `cmd` is not claude.
pub fn enforce(cmd: &[String], path: &OsStr, home: &str) -> Result<()> {
    let Some(claude) = claude_in(cmd, path) else {
        return Ok(());
    };
    match verdict(&claude, path, home) {
        Verdict::Supported(_) => Ok(()),
        Verdict::Below(version) => Err(Hinted::new(
            format!(
                "{} is Claude Code {version}, below {FLOOR}, which has a sandbox escape (CVE-2026-55607)",
                claude.display()
            ),
            update_hint(),
        )
        .into()),
        Verdict::Unreadable => Err(Hinted::new(
            format!(
                "could not read the version of {}; {FLOOR} or newer is required",
                claude.display()
            ),
            format!(
                "run `{} --version` to see why it fails or hangs, then {}",
                claude.display(),
                update_hint()
            ),
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::{Scratch, write_program};

    #[test]
    fn parses_the_version_banner() {
        assert_eq!(parse("2.1.163 (Claude Code)\n"), Some(Version(2, 1, 163)));
        assert_eq!(parse("claude v2.0.9-beta.1"), Some(Version(2, 0, 9)));
        assert_eq!(parse("no version here"), None);
        assert_eq!(parse("2.1"), None);
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(Version(2, 1, 162) < FLOOR);
        assert!(Version(2, 1, 1000) > FLOOR);
        assert!(Version(2, 2, 0) > FLOOR);
        assert!(Version(10, 0, 0) > FLOOR);
    }

    #[test]
    fn judges_a_program_by_its_banner() {
        let scratch = Scratch::new("claude-version");
        let bin = scratch.path().join("bin");
        write_program(&bin, "old", "echo '2.1.162 (Claude Code)'");
        write_program(&bin, "new", "echo '2.1.163 (Claude Code)'");
        write_program(&bin, "odd", "echo hello");
        let judge = |name: &str| verdict(&bin.join(name), bin.as_os_str(), "/nonexistent");
        assert_eq!(judge("old"), Verdict::Below(Version(2, 1, 162)));
        assert_eq!(judge("new"), Verdict::Supported(FLOOR));
        assert_eq!(judge("odd"), Verdict::Unreadable);
        assert_eq!(judge("missing"), Verdict::Unreadable);
    }

    #[test]
    fn enforce_refuses_an_unreadable_version() {
        let scratch = Scratch::new("claude-unreadable");
        let bin = scratch.path().join("bin");
        write_program(&bin, "claude", "echo hello");
        let by_name = ["claude".to_string()];
        let err = enforce(&by_name, bin.as_os_str(), "/nonexistent").unwrap_err();
        assert!(err.to_string().contains("could not read the version"));
        let hint = capsa_core::hint::find_hint(&err).unwrap();
        assert!(
            hint.contains("--version") && hint.contains("claude update"),
            "{hint}"
        );
    }

    #[test]
    fn enforce_refuses_below_the_floor_and_ignores_other_programs() {
        let scratch = Scratch::new("claude-enforce");
        let bin = scratch.path().join("bin");
        write_program(&bin, "claude", "echo '2.0.0 (Claude Code)'");
        let path = bin.as_os_str();
        let by_name = ["claude".to_string()];
        assert!(enforce(&by_name, path, "/nonexistent").is_err());
        let by_path = [bin.join("claude").display().to_string()];
        assert!(enforce(&by_path, path, "/nonexistent").is_err());
        let other = ["sh".to_string(), "-c".to_string(), "true".to_string()];
        assert!(enforce(&other, path, "/nonexistent").is_ok());
    }
}

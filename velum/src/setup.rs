// SPDX-License-Identifier: MPL-2.0
//! Owns `velum self setup`: the default policy file and the srt install.

use crate::policy::DEFAULT_POLICY_TOML;
use crate::srt;
use anyhow::{Context, Result, bail};
use capsa_core::hint::{Hinted, ResultExt};
use capsa_core::pkg::{self, Manager};
use capsa_core::which;
use clap::ValueEnum;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

const PACKAGE: &str = "@anthropic-ai/sandbox-runtime";

/// The package manager that installs srt.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PackageManager {
    Pnpm,
    Npm,
    Bun,
}

impl From<PackageManager> for Manager {
    fn from(package_manager: PackageManager) -> Self {
        match package_manager {
            PackageManager::Pnpm => Self::Pnpm,
            PackageManager::Npm => Self::Npm,
            PackageManager::Bun => Self::Bun,
        }
    }
}

/// Writes the default policy if it is missing, then installs srt unless it can already be
/// found (on PATH or in a user bin directory). After an install it checks that srt is
/// findable, and says so when its directory is not on PATH.
pub fn run(
    package_manager: PackageManager,
    dry_run: bool,
    policy_path: &Path,
    home: &str,
) -> Result<()> {
    write_default_policy(policy_path, dry_run)?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(srt_bin) = srt::find_srt(home, &path) {
        let version = srt_version(&srt_bin);
        println!(
            "srt {} at {}",
            version.as_deref().unwrap_or("(version unknown)"),
            srt_bin.display()
        );
        print_path_note(&srt_bin, &path);
        return Ok(());
    }
    let manager = Manager::from(package_manager);
    let command = pkg::install_command(manager, PACKAGE).join(" ");
    println!("srt not found; install with: {command}");
    if dry_run {
        return Ok(());
    }
    let report = pkg::install(manager, PACKAGE, false).hinted()?;
    if let Some(status) = report.status.filter(|s| !s.success()) {
        bail!("{command} failed: {status}");
    }
    let srt_bin = installed_srt(home, &path)?;
    println!("installed srt at {}", srt_bin.display());
    print_path_note(&srt_bin, &path);
    Ok(())
}

/// Returns the srt that an install put in place, or an error with a hint when it cannot be found.
fn installed_srt(home: &str, path: &OsStr) -> Result<PathBuf> {
    srt::find_srt(home, path).ok_or_else(|| {
        Hinted::new(
            "srt was installed but velum cannot find it on PATH or in the user bin directories",
            "find the package manager's global bin directory (pnpm v12: ~/Library/pnpm/bin; run `pnpm setup`) and add it to PATH",
        )
        .into()
    })
}

fn print_path_note(srt_bin: &Path, path: &OsStr) {
    if let Some(note) = path_note(srt_bin, path) {
        println!("{note}");
    }
}

/// Returns a note when the directory of `srt_bin` is not on PATH: velum finds srt there anyway.
fn path_note(srt_bin: &Path, path: &OsStr) -> Option<String> {
    let dir = srt_bin.parent()?;
    which::path_index(path, dir).is_none().then(|| {
        format!(
            "note: {} is not on PATH; velum finds srt there. To run `srt` yourself, add it to PATH \
             (pnpm v12's global bin is ~/Library/pnpm/bin: run `pnpm setup` or add it to your shell profile)",
            dir.display()
        )
    })
}

fn write_default_policy(path: &Path, dry_run: bool) -> Result<()> {
    if path.exists() {
        println!("policy exists, left unchanged: {}", path.display());
        return Ok(());
    }
    if dry_run {
        println!("would write {}:\n{DEFAULT_POLICY_TOML}", path.display());
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(path, DEFAULT_POLICY_TOML)
        .with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

/// Returns the version `srt --version` prints, when it runs.
pub fn srt_version(bin: &Path) -> Option<String> {
    Command::new(bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::{Scratch, write_program};

    #[test]
    fn a_dir_off_path_gets_a_note_naming_it() {
        let bin = Path::new("/Users/a/Library/pnpm/bin/srt");
        let note = path_note(bin, OsStr::new("/usr/bin:/bin")).unwrap();
        assert!(note.contains("/Users/a/Library/pnpm/bin"));
        assert!(note.contains("pnpm setup"));
        let on_path = OsStr::new("/usr/bin:/Users/a/Library/pnpm/bin");
        assert!(path_note(bin, on_path).is_none());
    }

    #[test]
    fn verification_finds_srt_in_the_pnpm_dir_and_fails_with_a_hint_otherwise() {
        let scratch = Scratch::new("setup-verify");
        let home = scratch.path().join("home");
        let home = home.to_str().unwrap();
        let err = installed_srt(home, OsStr::new("")).map(drop);
        // A real srt in /opt/homebrew/bin or /usr/local/bin may exist on a dev machine.
        if let Err(err) = err {
            assert!(capsa_core::hint::find_hint(&err).is_some());
        }
        write_program(
            &Path::new(home).join("Library/pnpm/bin"),
            "srt",
            "echo 0.0.80",
        );
        let found = installed_srt(home, OsStr::new("")).unwrap();
        assert!(found.ends_with("Library/pnpm/bin/srt"));
    }
}

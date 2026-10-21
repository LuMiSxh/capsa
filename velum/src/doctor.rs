// SPDX-License-Identifier: MPL-2.0
//! Owns the `velum self doctor` checks: srt, the policy, the settings, claude and the shim.

use crate::check::{Check, Status, report};
use crate::claude_version::{self, Verdict};
use crate::policy::{Policy, PolicyError, default_path};
use crate::{safedir, setup, shim, srt, writable};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::which;
use serde_json::Value;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Settings keys that srt needs, as JSON pointers.
const REQUIRED_SETTINGS: &[&str] = &[
    "/network/allowedDomains",
    "/network/deniedDomains",
    "/filesystem/denyRead",
    "/filesystem/allowWrite",
    "/filesystem/denyWrite",
];

/// Runs the checks on the real environment, prints them in `format`, and fails when one fails.
pub fn run(format: Format) -> Result<Outcome> {
    let home = crate::home()?;
    let cwd = std::env::current_dir().context("reading current directory")?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let tmp_dirs = srt::TMP_DIRS.map(PathBuf::from);
    report(format, &checks(&home, &cwd, &path, &tmp_dirs))
}

/// Returns every check, in the order doctor prints them. The `tmp_dirs` are created and probed.
pub fn checks(home: &str, cwd: &Path, path: &OsStr, tmp_dirs: &[PathBuf]) -> Vec<Check> {
    let file = default_path(home);
    let loaded = Policy::load(&file, cwd);
    vec![
        srt_check(home, path),
        policy_check(&file, loaded.as_ref()),
        settings_check(loaded.as_ref(), home),
        trust_check(home, cwd, path),
        env_check(loaded.as_ref()),
        claude_check(path),
        shim::position_check(&shim::default_dir(home), path),
    ]
    .into_iter()
    .chain(shim::velum_check(&shim::default_dir(home)))
    .chain(claude_version_check(home, path))
    .chain([bare_repository_check(home), tmp_check(tmp_dirs)])
    .collect()
}

/// Checks the real claude (not the shim) against `claude_version::FLOOR`. Absent when no
/// claude is found; the `claude` check already fails then.
fn claude_version_check(home: &str, path: &OsStr) -> Option<Check> {
    let real = shim::real_claude(&shim::default_dir(home), path).ok()?;
    let floor = claude_version::FLOOR;
    Some(match claude_version::verdict(&real, path, home) {
        Verdict::Supported(version) => Check::new(
            "claude version",
            Status::Ok,
            format!("{version} at {} (floor {floor})", real.display()),
        ),
        Verdict::Below(version) => Check::new(
            "claude version",
            Status::Fail,
            format!(
                "{version} at {} is below {floor} (CVE-2026-55607, sandbox escape); hint: {}",
                real.display(),
                claude_version::update_hint()
            ),
        ),
        Verdict::Unreadable => Check::new(
            "claude version",
            Status::Warn,
            format!(
                "could not read the version of {}; {floor} or newer is required",
                real.display()
            ),
        ),
    })
}

/// Warns unless the global git config sets `safe.bareRepository` to `explicit`, so a bare
/// repository planted in a working directory is not opened implicitly. Never edits the config.
fn bare_repository_check(home: &str) -> Check {
    let output = std::process::Command::new("git")
        .args(["config", "--global", "--get", "safe.bareRepository"])
        .env("HOME", home)
        .output();
    let fix = "hint: run `git config --global safe.bareRepository explicit`";
    match output {
        Err(_) => Check::new(
            "git bare repositories",
            Status::Warn,
            "git not found; safe.bareRepository not checked",
        ),
        Ok(out) if String::from_utf8_lossy(&out.stdout).trim() == "explicit" => Check::new(
            "git bare repositories",
            Status::Ok,
            "safe.bareRepository is explicit",
        ),
        Ok(out) => {
            let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let state = if value.is_empty() { "not set" } else { &value };
            Check::new(
                "git bare repositories",
                Status::Warn,
                format!("safe.bareRepository is {state}, not explicit; {fix}"),
            )
        }
    }
}

/// Warns when srt, the real claude or the velum binary could be written by the sandbox
/// started in `cwd`: whoever can write them runs code outside the sandbox.
pub fn trust_check(home: &str, cwd: &Path, path: &OsStr) -> Check {
    let loaded = Policy::load(&default_path(home), cwd);
    let Ok(settings) = loaded
        .as_ref()
        .map_err(ToString::to_string)
        .and_then(|policy| srt::settings(policy, home).map_err(|err| err.to_string()))
    else {
        return Check::new(
            "trust",
            Status::Warn,
            "the policy does not load; not checked",
        );
    };
    let velum = std::env::current_exe().and_then(std::fs::canonicalize).ok();
    let real_claude = shim::real_claude(&shim::default_dir(home), path).ok();
    let items = [
        ("srt", srt::find_srt(home, path)),
        ("claude", real_claude),
        ("velum", velum),
    ];
    let writable: Vec<String> = items
        .iter()
        .filter_map(|(what, found)| Some((what, found.as_ref()?)))
        .filter_map(|(what, found)| writable::ensure_not_writable(&settings, what, found).err())
        .map(|err| err.to_string())
        .collect();
    if writable.is_empty() {
        Check::new(
            "trust",
            Status::Ok,
            "srt, claude and velum are outside every writable path",
        )
    } else {
        Check::new("trust", Status::Warn, writable.join("; "))
    }
}

/// Notes that secret-looking variables are withheld from the child.
fn env_check(policy: Result<&Policy, &PolicyError>) -> Check {
    let Ok(policy) = policy else {
        return Check::new("env", Status::Warn, "the policy does not load; not checked");
    };
    let names = std::env::vars_os().map(|(name, _)| name.to_string_lossy().into_owned());
    let withheld = srt::withheld_secrets(policy, names);
    let rule = "names containing TOKEN, SECRET, PASSWORD, PASSWD or CREDENTIAL, or ending in _KEY, \
                are dropped even when listed in env.pass; list one exactly in env.pass_secret to pass it";
    let detail = if withheld.is_empty() {
        format!("{rule}; none set now")
    } else {
        format!("{rule}; not passed now: {}", withheld.join(", "))
    };
    Check::new("env", Status::Ok, detail)
}

fn srt_check(home: &str, path: &OsStr) -> Check {
    let Some(bin) = srt::find_srt(home, path) else {
        return Check::new(
            "srt",
            Status::Fail,
            "not found on PATH or in the user bin directories; run velum self setup",
        );
    };
    match setup::srt_version(&bin) {
        Some(version) => Check::new("srt", Status::Ok, format!("{} {version}", bin.display())),
        None => Check::new(
            "srt",
            Status::Warn,
            format!("{} found, but srt --version failed", bin.display()),
        ),
    }
}

fn policy_check(file: &Path, loaded: Result<&Policy, &PolicyError>) -> Check {
    let shown = file.display();
    match loaded {
        Err(err) => Check::new("policy", Status::Fail, err.to_string()),
        Ok(_) if file.exists() => Check::new(
            "policy",
            Status::Ok,
            format!("{shown} parses; layers: built-in defaults, file"),
        ),
        Ok(_) => Check::new(
            "policy",
            Status::Ok,
            format!("no file at {shown}; layers: built-in defaults"),
        ),
    }
}

fn settings_check(policy: Result<&Policy, &PolicyError>, home: &str) -> Check {
    let Ok(policy) = policy else {
        let detail = "the policy does not load, so there are no settings";
        return Check::new("settings", Status::Fail, detail);
    };
    let settings = match srt::settings(policy, home) {
        Ok(settings) => settings,
        Err(err) => return Check::new("settings", Status::Fail, err.to_string()),
    };
    let missing: Vec<&str> = REQUIRED_SETTINGS
        .iter()
        .copied()
        .filter(|key| !settings.pointer(key).is_some_and(Value::is_array))
        .collect();
    if missing.is_empty() {
        let count = REQUIRED_SETTINGS.len();
        let detail = format!("srt settings valid; {count} required keys present");
        Check::new("settings", Status::Ok, detail)
    } else {
        Check::new(
            "settings",
            Status::Fail,
            format!("missing {}", missing.join(", ")),
        )
    }
}

fn claude_check(path: &OsStr) -> Check {
    let Some((_, claude)) = which::find_on_path("claude", path, None) else {
        return Check::new("claude", Status::Fail, "not found on PATH");
    };
    if shim::is_shim(&claude) {
        Check::new(
            "claude",
            Status::Ok,
            format!("{} is the velum shim", claude.display()),
        )
    } else {
        Check::new(
            "claude",
            Status::Warn,
            format!("{} runs first and is not the velum shim", claude.display()),
        )
    }
}

/// The temporary directories are created and written to, as `velum run` sets them up.
fn tmp_check(dirs: &[PathBuf]) -> Check {
    let shown: Vec<String> = dirs.iter().map(|dir| dir.display().to_string()).collect();
    for dir in dirs {
        if let Err(err) = probe(dir) {
            return Check::new(
                "TMPDIR",
                Status::Fail,
                format!("{}: {err:#}", dir.display()),
            );
        }
    }
    Check::new(
        "TMPDIR",
        Status::Ok,
        format!("{} writable", shown.join(", ")),
    )
}

fn probe(dir: &Path) -> Result<()> {
    safedir::ensure_private_dir(dir)?;
    let file = dir.join(format!(".doctor-{}", std::process::id()));
    std::fs::write(&file, b"").and_then(|()| std::fs::remove_file(&file))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::outcome;
    use crate::scratch::{Scratch, write_program};

    fn run_checks(scratch: &Scratch, path: &OsStr) -> Vec<Check> {
        let home = scratch.path().join("home");
        let tmp = [scratch.path().join("tmp"), scratch.path().join("tmp2")];
        checks(home.to_str().unwrap(), scratch.path(), path, &tmp)
    }

    fn find<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks.iter().find(|check| check.name == name).unwrap()
    }

    #[test]
    fn healthy_install_fails_only_on_the_missing_claude() {
        let scratch = Scratch::new("doctor-healthy");
        let bin = scratch.path().join("bin");
        write_program(&bin, "srt", "echo 0.0.81");
        let found = run_checks(&scratch, bin.as_os_str());
        assert_eq!(find(&found, "srt").status, Status::Ok);
        assert!(find(&found, "srt").detail.contains("0.0.81"));
        assert_eq!(find(&found, "policy").status, Status::Ok);
        assert_eq!(find(&found, "settings").status, Status::Ok);
        assert_eq!(find(&found, "claude").status, Status::Fail);
        assert_eq!(find(&found, "shim dir").status, Status::Warn);
        assert_eq!(find(&found, "TMPDIR").status, Status::Ok);
        assert_eq!(find(&found, "env").status, Status::Ok);
        assert!(find(&found, "env").detail.contains("env.pass_secret"));
        assert_eq!(outcome(&found), Outcome::Code(1));
    }

    #[test]
    fn a_trusted_binary_in_a_writable_directory_is_warned_about() {
        let scratch = Scratch::new("doctor-trust");
        let bin = scratch.path().join("bin");
        write_program(&bin, "srt", "echo 0.0.80");
        // The working directory is scratch, so `bin` lies under allowWrite.
        let found = run_checks(&scratch, bin.as_os_str());
        let trust = find(&found, "trust");
        assert_eq!(trust.status, Status::Warn);
        assert!(trust.detail.contains("srt"), "{}", trust.detail);
    }

    #[test]
    fn srt_in_the_pnpm_bin_dir_is_found_without_path() {
        let scratch = Scratch::new("doctor-pnpm");
        let home = scratch.path().join("home");
        write_program(&home.join("Library/pnpm/bin"), "srt", "echo 0.0.80");
        let cwd = scratch.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        let tmp = [scratch.path().join("t")];
        let found = checks(home.to_str().unwrap(), &cwd, OsStr::new(""), &tmp);
        assert_eq!(find(&found, "srt").status, Status::Ok);
        assert!(find(&found, "srt").detail.contains("Library/pnpm/bin/srt"));
    }

    #[test]
    fn a_policy_that_does_not_parse_fails_policy_and_settings() {
        let scratch = Scratch::new("doctor-policy");
        let file = scratch.path().join("home/.config/velum/policy.toml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "[fs\n").unwrap();
        let found = run_checks(&scratch, OsStr::new(""));
        assert_eq!(find(&found, "policy").status, Status::Fail);
        assert!(find(&found, "policy").detail.contains("parsing"));
        assert_eq!(find(&found, "settings").status, Status::Fail);
    }

    #[test]
    fn claude_below_the_floor_fails_and_unreadable_warns() {
        let scratch = Scratch::new("doctor-version");
        let bin = scratch.path().join("bin");
        write_program(&bin, "claude", "echo '2.1.162 (Claude Code)'");
        let found = run_checks(&scratch, bin.as_os_str());
        let check = find(&found, "claude version");
        assert_eq!(check.status, Status::Fail);
        assert!(check.detail.contains("claude update"), "{}", check.detail);
        assert_eq!(outcome(&found), Outcome::Code(1));
        write_program(&bin, "claude", "echo 2.1.163");
        let found = run_checks(&scratch, bin.as_os_str());
        assert_eq!(find(&found, "claude version").status, Status::Ok);
        write_program(&bin, "claude", "echo unknown");
        let found = run_checks(&scratch, bin.as_os_str());
        assert_eq!(find(&found, "claude version").status, Status::Warn);
    }

    #[test]
    fn safe_bare_repository_must_be_explicit() {
        let scratch = Scratch::new("doctor-bare");
        let home = scratch.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let home = home.to_str().unwrap();
        let unset = bare_repository_check(home);
        assert_eq!(unset.status, Status::Warn);
        assert!(unset.detail.contains("safe.bareRepository explicit"));
        std::fs::write(
            Path::new(home).join(".gitconfig"),
            "[safe]\n\tbareRepository = explicit\n",
        )
        .unwrap();
        assert_eq!(bare_repository_check(home).status, Status::Ok);
        std::fs::write(
            Path::new(home).join(".gitconfig"),
            "[safe]\n\tbareRepository = all\n",
        )
        .unwrap();
        assert_eq!(bare_repository_check(home).status, Status::Warn);
    }

    #[test]
    fn a_velum_shim_first_on_path_is_recognised() {
        let scratch = Scratch::new("doctor-shim");
        let bin = scratch.path().join("bin");
        write_program(&bin, "claude", "# velum shim");
        let found = run_checks(&scratch, bin.as_os_str());
        assert_eq!(find(&found, "claude").status, Status::Ok);
        assert!(find(&found, "claude").detail.contains("velum shim"));
    }
}

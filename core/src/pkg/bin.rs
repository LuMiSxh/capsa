// SPDX-License-Identifier: MPL-2.0
//! Owns the global binary directory of each package manager, asked of the manager.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::Manager;
use crate::which::{self, Env, Fs, Platform, RealFs};

/// How long a package manager may take to name its global binary directory.
const ASK_TIMEOUT: Duration = Duration::from_secs(10);

/// Returns the directory where `manager` puts global binaries. The manager is
/// asked first, then `PNPM_HOME` or the home directory is tried. `None` when
/// neither answers.
pub fn global_bin_dir(manager: Manager) -> Option<PathBuf> {
    let env = Env::from_process();
    let pnpm_home = std::env::var_os("PNPM_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let run = |program: &str, args: &[&str]| run_output(&env, &RealFs, program, args, ASK_TIMEOUT);
    global_bin_dir_in(&env, &RealFs, manager, pnpm_home.as_deref(), &run)
}

/// Returns the directory like [`global_bin_dir`], over the given environment, file system,
/// `PNPM_HOME` and command runner. `run` gets a program and its arguments and returns the
/// standard output of a run that exits with 0.
pub fn global_bin_dir_in(
    env: &Env,
    fs: &dyn Fs,
    manager: Manager,
    pnpm_home: Option<&Path>,
    run: &dyn Fn(&str, &[&str]) -> Option<String>,
) -> Option<PathBuf> {
    let asked = match manager {
        Manager::Pnpm => run("pnpm", &["bin", "-g"]).map(|out| PathBuf::from(out.trim())),
        Manager::Bun => run("bun", &["pm", "bin", "-g"]).map(|out| PathBuf::from(out.trim())),
        Manager::Npm => run("npm", &["prefix", "-g"]).map(|out| npm_bin(env, out.trim())),
    };
    asked
        .filter(|dir| !dir.as_os_str().is_empty())
        .or_else(|| fallback(env, fs, manager, pnpm_home))
}

/// Returns where npm keeps global binaries under `prefix`: in `bin`, except on Windows, where
/// the prefix holds them.
fn npm_bin(env: &Env, prefix: &str) -> PathBuf {
    let prefix = PathBuf::from(prefix);
    if env.platform == Platform::Windows {
        prefix
    } else {
        prefix.join("bin")
    }
}

/// Returns the first existing directory a manager uses when it does not answer:
/// `PNPM_HOME/bin` then `PNPM_HOME` for pnpm (pnpm 11 moved binaries into
/// `bin`), and `~/.bun/bin` for bun.
fn fallback(env: &Env, fs: &dyn Fs, manager: Manager, pnpm_home: Option<&Path>) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = match manager {
        Manager::Pnpm => pnpm_home
            .into_iter()
            .flat_map(|home| [home.join("bin"), home.to_path_buf()])
            .collect(),
        Manager::Bun => env
            .home
            .as_ref()
            .map(|home| home.join(".bun").join("bin"))
            .into_iter()
            .collect(),
        Manager::Npm => Vec::new(),
    };
    candidates.into_iter().find(|dir| fs.is_dir(dir))
}

/// Runs `program` as `env`'s PATH finds it, with its own directory first on the
/// child's PATH, and returns its standard output when it exits with 0 within `timeout`. A run
/// that takes longer is killed and gives `None`, like one that fails.
fn run_output(
    env: &Env,
    fs: &dyn Fs,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Option<String> {
    let found = which::find_in(env, fs, program, None)?;
    let mut command = Command::new(&found);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Ok(path) = std::env::join_paths(which::child_path(env, fs, &found)) {
        command.env("PATH", path);
    }
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    status.success().then_some(text)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn program(dir: &Path, name: &str, body: &str) -> Env {
        let file = dir.join(name);
        std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        Env {
            platform: Platform::Linux,
            path: vec![dir.to_path_buf()],
            home: None,
            appdata: None,
            local_appdata: None,
            pathext: Vec::new(),
        }
    }

    #[test]
    fn a_hung_manager_is_killed_and_gives_none() {
        let dir = tempfile::tempdir().unwrap();
        let env = program(dir.path(), "hang", "exec sleep 30");
        let started = Instant::now();
        let out = run_output(&env, &RealFs, "hang", &[], Duration::from_millis(200));
        assert_eq!(out, None);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn an_answering_manager_gives_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let env = program(dir.path(), "ok", "echo /some/bin");
        let out = run_output(&env, &RealFs, "ok", &[], Duration::from_secs(10));
        assert_eq!(out.as_deref(), Some("/some/bin\n"));
        let env = program(dir.path(), "bad", "echo /x; exit 1");
        assert_eq!(
            run_output(&env, &RealFs, "bad", &[], Duration::from_secs(10)),
            None
        );
    }
}

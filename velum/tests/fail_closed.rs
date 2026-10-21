// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "the helpers are test code")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway layout under the target directory, which no default `allowWrite` entry covers:
/// `home/`, `work/` (the working directory) and `bin/` (fake programs).
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fail-closed-{name}"));
        let _ = fs::remove_dir_all(&root);
        for dir in ["home", "work", "bin"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        Self(root)
    }

    fn program(&self, name: &str, body: &str) {
        let file = self.0.join("bin").join(name);
        fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn marker(&self, name: &str) -> PathBuf {
        self.0.join("work").join(name)
    }

    /// Runs `velum run -- cmd` with a clean environment and PATH limited to `bin/`.
    fn velum(&self, path: &str, cmd: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_velum"))
            .args(["run", "--"])
            .args(cmd)
            .env_clear()
            .env("HOME", self.0.join("home"))
            .env("PATH", path)
            .current_dir(self.0.join("work"))
            .output()
            .unwrap()
    }

    fn bin_path(&self) -> String {
        self.0.join("bin").display().to_string()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn refuses_to_run_without_srt() {
    let sandbox = Sandbox::new("no-srt");
    let marker = sandbox.marker("ran");
    let script = format!("/usr/bin/touch {}", marker.display());
    let out = sandbox.velum(&sandbox.bin_path(), &["/bin/sh", "-c", &script]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("srt not found"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("velum self setup"),
        "{}",
        stderr(&out)
    );
    assert!(!marker.exists(), "the command ran without srt");
}

#[test]
fn does_not_run_the_command_when_srt_fails_at_once() {
    let sandbox = Sandbox::new("srt-fails");
    sandbox.program("srt", "echo 'srt: boom' >&2; exit 3");
    let marker = sandbox.marker("ran");
    let script = format!("/usr/bin/touch {}", marker.display());
    let out = sandbox.velum(&sandbox.bin_path(), &["/bin/sh", "-c", &script]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(!marker.exists(), "the command ran after srt failed");
}

#[test]
fn does_not_start_srt_when_the_settings_cannot_be_written() {
    let sandbox = Sandbox::new("no-settings");
    let started = sandbox.marker("srt-started");
    sandbox.program("srt", &format!("/usr/bin/touch {}", started.display()));
    // A file where the run directories belong makes the run directory impossible to create.
    let support = sandbox.0.join("home/Library/Application Support");
    fs::create_dir_all(&support).unwrap();
    fs::write(support.join("velum"), "").unwrap();
    let out = sandbox.velum(&sandbox.bin_path(), &["/bin/sh", "-c", "true"]);
    assert!(!out.status.success());
    assert!(!started.exists(), "srt started without a settings file");
}

#[test]
fn refuses_a_claude_below_the_version_floor() {
    let sandbox = Sandbox::new("old-claude");
    let started = sandbox.marker("srt-started");
    sandbox.program("srt", &format!("/usr/bin/touch {}", started.display()));
    sandbox.program("claude", "echo '2.1.162 (Claude Code)'");
    let out = sandbox.velum(&sandbox.bin_path(), &["claude"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("below 2.1.163"), "{}", stderr(&out));
    assert!(stderr(&out).contains("claude update"), "{}", stderr(&out));
    assert!(!started.exists(), "srt started for an old claude");
}

#[test]
fn starts_srt_for_a_claude_at_the_floor() {
    let sandbox = Sandbox::new("new-claude");
    let started = sandbox.marker("srt-started");
    sandbox.program("srt", &format!("/usr/bin/touch {}", started.display()));
    sandbox.program("claude", "echo '2.1.163 (Claude Code)'");
    let out = sandbox.velum(&sandbox.bin_path(), &["claude"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(started.exists());
}

#[test]
fn refuses_a_claude_whose_version_is_unreadable() {
    let sandbox = Sandbox::new("odd-claude");
    let started = sandbox.marker("srt-started");
    sandbox.program("srt", &format!("/usr/bin/touch {}", started.display()));
    sandbox.program("claude", "echo hello");
    let out = sandbox.velum(&sandbox.bin_path(), &["claude"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("could not read the version"),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("claude update"), "{}", stderr(&out));
    assert!(!started.exists(), "srt started for an unreadable claude");
}

#[test]
fn refuses_a_claude_whose_version_check_hangs() {
    let sandbox = Sandbox::new("hung-claude");
    let started = sandbox.marker("srt-started");
    sandbox.program("srt", &format!("/usr/bin/touch {}", started.display()));
    sandbox.program("claude", "exec /bin/sleep 60");
    let out = sandbox.velum(&sandbox.bin_path(), &["claude"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("could not read the version"),
        "{}",
        stderr(&out)
    );
    assert!(!started.exists(), "srt started for a hung claude");
}

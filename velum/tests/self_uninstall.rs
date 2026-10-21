// SPDX-License-Identifier: MPL-2.0
#![expect(clippy::unwrap_used, reason = "the helpers are test code")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway home with a copy of the velum binary beside it, so uninstalling removes the copy
/// and never the built binary.
struct Install {
    root: PathBuf,
    copy: PathBuf,
}

impl Install {
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("self-uninstall-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home/.local/bin")).unwrap();
        fs::create_dir_all(root.join("home/.config/velum")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        let copy = root.join("bin/velum");
        fs::copy(env!("CARGO_BIN_EXE_velum"), &copy).unwrap();
        Self { root, copy }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn claude(&self) -> PathBuf {
        self.home().join(".local/bin/claude")
    }

    fn policy(&self) -> PathBuf {
        self.home().join(".config/velum/policy.toml")
    }

    fn velum(&self, args: &[&str]) -> Output {
        Command::new(&self.copy)
            .args(["self", "uninstall"])
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .output()
            .unwrap()
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn removes_the_shim_and_the_binary_and_keeps_the_policy() {
    let install = Install::new("keep");
    fs::write(install.claude(), "#!/bin/sh\n# velum shim\nexit 0\n").unwrap();
    fs::write(install.policy(), "# mine\n").unwrap();
    let out = install.velum(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!install.claude().exists());
    assert!(!install.copy.exists());
    assert!(install.policy().is_file());
    assert!(stdout(&out).contains("--purge"), "{}", stdout(&out));
}

#[test]
fn purge_removes_the_policy_file_too() {
    let install = Install::new("purge");
    fs::write(install.policy(), "# mine\n").unwrap();
    let out = install.velum(&["--purge"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!install.policy().exists());
    assert!(!install.copy.exists());
}

#[test]
fn dry_run_removes_nothing() {
    let install = Install::new("dry");
    fs::write(install.claude(), "#!/bin/sh\n# velum shim\n").unwrap();
    fs::write(install.policy(), "# mine\n").unwrap();
    let out = install.velum(&["--dry-run", "--purge"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = stdout(&out);
    assert_eq!(text.matches("would remove").count(), 3, "{text}");
    assert!(install.claude().is_file() && install.policy().is_file() && install.copy.is_file());
}

#[test]
fn a_claude_that_is_not_the_shim_is_left_alone() {
    let install = Install::new("foreign");
    fs::write(install.claude(), "#!/bin/sh\necho real claude\n").unwrap();
    let out = install.velum(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(install.claude().is_file());
    assert!(!install.copy.exists());
    assert!(
        stdout(&out).contains("not a velum shim"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn refuses_a_binary_not_named_velum_and_removes_nothing() {
    let install = Install::new("renamed");
    fs::write(install.claude(), "#!/bin/sh\n# velum shim\n").unwrap();
    let renamed = install.root.join("bin/other");
    fs::rename(&install.copy, &renamed).unwrap();
    let out = Command::new(&renamed)
        .args(["self", "uninstall"])
        .env_clear()
        .env("HOME", install.home())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(install.claude().is_file());
    assert!(renamed.is_file());
}

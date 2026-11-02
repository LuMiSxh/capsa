// SPDX-License-Identifier: MPL-2.0
use std::process::Command;

/// Settings for the snapshots in `tests/snapshots`. They keep the names the single
/// `cli.rs` binary gave them (`cli__<name>`), so the names are passed explicitly.
pub fn snapshot_settings() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_prepend_module_to_snapshot(false);
    settings.set_snapshot_path("../snapshots");
    settings
}

/// The binary with any user config hidden.
pub fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_acus"));
    c.env("ACUS_CONFIG", "/nonexistent/acus.toml")
        .env_remove("ACUS_DISABLE");
    c
}

pub fn acus(args: &[&str]) -> (i32, String, String) {
    acus_env(args, &[])
}

pub fn acus_env(args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    decode(
        bin()
            .args(args)
            .envs(env.iter().copied())
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"))
            .output()
            .unwrap(),
    )
}

pub fn acus_in(dir: &std::path::Path, args: &[&str], stdin: &str) -> (i32, String, String) {
    acus_in_env(dir, args, stdin, &[])
}

pub fn acus_in_env(
    dir: &std::path::Path,
    args: &[&str],
    stdin: &str,
    env: &[(&str, &str)],
) -> (i32, String, String) {
    use std::io::Write;
    let mut child = bin()
        .args(args)
        .envs(env.iter().copied())
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    decode(child.wait_with_output().unwrap())
}

/// Splits a finished process into exit code, stdout and stderr.
pub fn decode(out: std::process::Output) -> (i32, String, String) {
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// Runs `cmd` to completion. Linux reports a just-copied binary as busy (ETXTBSY) while another
/// test thread's fork still holds the copy's write descriptor, so the spawn is retried briefly.
#[cfg(feature = "cmd-update")]
pub fn output_retrying_busy(cmd: &mut Command) -> std::io::Result<std::process::Output> {
    for _ in 0..50 {
        match cmd.output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            other => return other,
        }
    }
    cmd.output()
}

/// `git ARGS` in `dir` with the test identity and no signing; panics with stderr when it fails.
pub fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Runs git in `dir` with a fixed author and date, so commit ids and blame dates are stable.
pub fn git_at(dir: &std::path::Path, date: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

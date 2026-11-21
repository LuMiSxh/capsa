// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

#[test]
fn show_clips_a_line_longer_than_the_character_budget() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("min.js"), "x".repeat(30_000)).unwrap();
    let (code, out, _) = acus_in(d.path(), &["show", "min.js"], "");
    assert_eq!(code, 0);
    assert!(out.contains("… (+"), "{out}");
    assert!(out.ends_with("rerun with --full\n"), "{out}");
    assert!(!out.contains("acus show min.js:1-1"), "{out}");
    assert!(out.chars().count() < 25_000, "{}", out.chars().count());
}

#[test]
fn commands_can_be_disabled_at_runtime() {
    let (code, _, err) = acus_env(&["find", "needle"], &[("ACUS_DISABLE", "log, find")]);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("error: acus find is disabled by configuration\nhint: "),
        "{err}"
    );
}

#[test]
fn invalid_config_is_reported() {
    let d = tempfile::tempdir().unwrap();
    let cfg = d.path().join("c.toml");
    std::fs::write(&cfg, "[commands]\ndisabld = []\n").unwrap();
    let (code, _, err) = acus_env(&["find", "x"], &[("ACUS_CONFIG", cfg.to_str().unwrap())]);
    assert_eq!(code, 2);
    assert!(err.contains("invalid config"), "{err}");
}

#[cfg(unix)]
#[test]
fn closed_stdout_ends_quietly() {
    use std::io::Read;
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("big.txt"), "line\n".repeat(50_000)).unwrap();
    let mut child = bin()
        .args(["show", "big.txt", "--full"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(d.path())
        .spawn()
        .unwrap();
    // Read a little, then close the pipe as `| head -1` does while output is still pending.
    let mut first = [0; 16];
    child.stdout.take().unwrap().read_exact(&mut first).unwrap();
    let status = child.wait().unwrap();
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert_ne!(status.code(), Some(101), "{err}");
    assert!(
        !err.contains("Broken pipe") && !err.contains("panicked"),
        "{err}"
    );
}

#[test]
fn skill_prints_body_and_find_path_errors_skip_regex_hint() {
    let (code, out, _) = acus(&["self", "skill"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("# acus"), "{out}");
    let (code, _, err) = acus(&["find", "x", "no/such/dir"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("path not found") && !err.contains("hint: escape"),
        "{err}"
    );
}

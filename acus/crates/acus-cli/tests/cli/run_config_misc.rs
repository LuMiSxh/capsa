// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

#[test]
fn run_survives_a_bad_line() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let (code, out, _) = acus_in(dir, &["run"], "find \"open\nshow src/lib.rs:1\n");
    assert_eq!(code, 2);
    assert!(
        out.contains("unclosed") && out.contains("== src/lib.rs 1-1"),
        "{out}"
    );
}

#[test]
fn run_batch() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let batch = r#"[["show","src/lib.rs#new"],["find","zzz_none"],["show","nope.rs"]]"#;
    let (code, out, _) = acus_in(dir, &["run"], batch);
    assert_eq!(code, 2);
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!("cli__run_batch", out);
    let (code, out, _) = acus_in(dir, &["run", "--json"], r#"[["show","src/lib.rs:1"]]"#);
    assert_eq!(
        (code, out.as_str()),
        (
            0,
            "[{\"address\":\"src/lib.rs\",\"start\":1,\"end\":1,\"text\":\"pub struct Parser {\"}]\n"
        )
    );
    let (code, out, err) = acus_in(dir, &["run", "--json"], r#"[["find","zzz_none"]]"#);
    assert_eq!((code, out.as_str(), err.as_str()), (1, "null\n", ""));
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
    let input = d.path().join("in.txt");
    std::fs::write(&input, "show src/lib.rs\n".repeat(5_000)).unwrap();
    let mut child = bin()
        .arg("run")
        .stdin(std::fs::File::open(&input).unwrap())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"))
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

#[test]
fn grep_flags_work_inside_run() {
    let (_, out, _) = acus_in(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj")),
        &["run"],
        "find needle -c --include *.rs\nfind -o need\\w+ -g *.rs\nshow src/lib.rs:10,11\n",
    );
    assert!(out.contains("src/lib.rs 3\ntotal 3 in 1 files"), "{out}");
    assert!(out.contains("11\tneedle"), "{out}");
    assert!(out.contains("10\t") && !out.contains("error"), "{out}");
}

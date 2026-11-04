// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

#[test]
fn run_survives_a_bad_line() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let (code, out, _) = acus_in(dir, &["run"], "find \"open\nshow src/lib.rs:1\n");
    assert_eq!(code, 0);
    assert!(
        out.contains("unclosed") && out.contains("== src/lib.rs 1-1"),
        "{out}"
    );
    assert!(
        out.ends_with("failed: 1 of 2 lines (line 1: unclosed \" in `find \"open`)\n"),
        "{out}"
    );
}

#[test]
fn run_batch() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let batch = r#"[["show","src/lib.rs#new"],["find","zzz_none"],["show","nope.rs"]]"#;
    let (code, out, _) = acus_in(dir, &["run"], batch);
    assert_eq!(code, 0);
    assert!(
        out.ends_with("failed: 1 of 3 lines (line 3: path not found: nope.rs)\n"),
        "{out}"
    );
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
fn run_json_keeps_one_line_per_entry() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let batch = r#"[["show","src/lib.rs:1"],["show","nope.rs"]]"#;
    let (code, out, _) = acus_in(dir, &["run", "--json"], batch);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 2, "{out}");
    assert!(
        out.lines()
            .nth(1)
            .is_some_and(|l| l.starts_with(r#"{"error":"#)),
        "{out}"
    );
}

#[test]
fn run_all_lines_failing_exits_two() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let (code, out, err) = acus_in(dir, &["run"], "show nope.rs\nshow nope.rs\n");
    assert_eq!(code, 2);
    assert!(
        out.contains(
            "failed: 2 of 2 lines (line 1: path not found: nope.rs, \
             line 2: path not found: nope.rs)"
        ),
        "{out}"
    );
    assert!(err.contains("error: all 2 lines failed"), "{err}");
}

#[test]
fn run_fail_fast_stops_at_the_first_failure() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let (code, out, err) = acus_in(
        dir,
        &["run", "--fail-fast"],
        "show src/lib.rs:1\nshow nope.rs\nshow src/lib.rs:2\n",
    );
    assert_eq!(code, 2);
    assert!(!out.contains(">>> acus show src/lib.rs:2"), "{out}");
    assert!(
        out.contains("failed: 1 of 3 lines (line 2: path not found: nope.rs)"),
        "{out}"
    );
    assert!(
        err.contains("error: --fail-fast stopped at line 2"),
        "{err}"
    );
}

fn big_file(d: &std::path::Path) {
    let pad = "x".repeat(60);
    let source: String = (1..=1000)
        .map(|i| format!("fn f{i}() {{ /* {pad} */ }}\n"))
        .collect();
    std::fs::write(d.join("big.rs"), source).unwrap();
}

#[test]
fn run_cuts_lines_fairly_and_names_the_rerun() {
    let d = tempfile::tempdir().unwrap();
    big_file(d.path());
    let (code, out, _) = acus_in(
        d.path(),
        &["run"],
        "show big.rs\nshow big.rs\nshow big.rs\n",
    );
    assert_eq!(code, 0);
    let cuts: Vec<usize> = out
        .match_indices("… cut at ")
        .filter_map(|(i, _)| {
            out[i + "… cut at ".len()..]
                .split(" chars")
                .next()?
                .parse()
                .ok()
        })
        .collect();
    assert_eq!(cuts.len(), 3, "{out}");
    for n in &cuts {
        assert!((7_000..=8_200).contains(n), "{cuts:?}\n{out}");
    }
    assert!(out.len() < 26_000, "{}", out.len());
    assert_eq!(
        out.matches("chars: acus show big.rs --head ").count(),
        3,
        "{out}"
    );
}

#[test]
fn run_full_lifts_the_budget() {
    let d = tempfile::tempdir().unwrap();
    big_file(d.path());
    let (code, out, _) = acus_in(
        d.path(),
        &["run", "--full"],
        "show big.rs:1-1000\nshow big.rs:1-1000\nshow big.rs:1-1000\n",
    );
    assert_eq!(code, 0);
    assert!(!out.contains("… cut at "), "{out}");
    assert!(out.len() > 60_000, "{}", out.len());
}

#[test]
fn run_head_does_not_override_a_line_head() {
    let d = tempfile::tempdir().unwrap();
    let source: String = (1..=30).map(|i| format!("fn f{i}() {{}}\n")).collect();
    std::fs::write(d.path().join("a.rs"), source).unwrap();
    let (code, out, _) = acus_in(d.path(), &["run", "--head", "2"], "show a.rs --head 5\n");
    assert_eq!(code, 0);
    assert!(out.contains("… output cut at --head 5;"), "{out}");
    assert!(!out.contains("--head 2"), "{out}");
}

#[test]
fn run_head_and_cut_cap_a_line_without_them() {
    let d = tempfile::tempdir().unwrap();
    let source: String = (1..=30).map(|i| format!("fn f{i}() {{}}\n")).collect();
    std::fs::write(d.path().join("a.rs"), source).unwrap();
    let (code, out, _) = acus_in(
        d.path(),
        &["run", "--head", "3", "--cut", "4"],
        "show a.rs\n",
    );
    assert_eq!(code, 0);
    assert!(out.contains("… output cut at --head 3;"), "{out}");
    assert!(out.contains("(+"), "{out}");
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

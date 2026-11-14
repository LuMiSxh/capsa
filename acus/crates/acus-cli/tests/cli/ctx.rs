// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

/// Lines `l1`-`l14` in order; `m3`, `m8` and `m14` match `^m`.
#[cfg(unix)]
const GREP_LINES: &str = "echo l1; echo l2; echo m3; echo l4; echo l5; echo l6; echo l7; echo m8; echo l9; echo l10; echo l11; echo l12; echo l13; echo m14";

#[test]
fn ctx_log_is_readable_with_show() {
    // The acus binary itself prints enough lines and exists on every platform.
    let cmd = format!("\"{}\" self skill", env!("CARGO_BIN_EXE_acus"));
    let (code, out, err) = acus(&["ctx", &cmd, "--max-output", "4"]);
    assert_eq!(code, 0, "{err}");
    let log = out
        .lines()
        .find_map(|l| l.split_once(", or acus show ")?.1.strip_suffix(":A-B"))
        .unwrap_or_else(|| panic!("no log path in {out}"));
    // On Windows the path starts with a drive letter, a colon the address parser must skip.
    let (code, shown, err) = acus(&["show", &format!("{log}:2-3")]);
    assert_eq!(code, 0, "{err}");
    assert!(shown.contains("2-3"), "{shown}");
    assert!(shown.lines().any(|l| l.starts_with("3\t")), "{shown}");
}

#[cfg(unix)]
#[test]
fn ctx_head_cuts_and_names_the_rerun() {
    let (code, out, _) = acus(&["ctx", "echo l1; echo l2; echo l3", "--head", "2"]);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "l1\nl2\n… output cut at --head 2; 2 more lines (rerun with --head 4)\n"
    );
}

#[cfg(unix)]
#[test]
fn ctx_tail_and_grep_keep_failures_and_their_code() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    boom()\n}\n").unwrap();
    let cmd =
        "echo l1; echo l2; echo 'error: boom at a.rs:2:5'; echo; echo l3; echo l4; echo l5; exit 3";
    let (code, out, _) = acus_in(d.path(), &["ctx", cmd, "--tail", "2"], "");
    assert_eq!(code, 3);
    assert!(
        out.starts_with("… 2 lines dropped (--tail 2)\nerror: boom at a.rs:2:5\n… 1 line dropped (--tail 2)\nl4\nl5\n"),
        "{out}"
    );
    assert!(out.contains("== a.rs#f (line 2)\n"), "{out}");
    assert!(out.contains("… full output (7 lines)"), "{out}");

    let (code, out, _) = acus_in(d.path(), &["ctx", cmd, "--grep", "^l[34]$"], "");
    assert_eq!(code, 3);
    assert!(
        out.starts_with("l3\nl4\n… 5 lines not matching --grep dropped\n"),
        "{out}"
    );
    assert!(!out.contains("a.rs#f"), "{out}");

    let (_, out, _) = acus_in(d.path(), &["ctx", cmd, "--grep", "^error"], "");
    assert!(out.contains("== a.rs#f (line 2)\n"), "{out}");

    let (code, _, err) = acus_in(d.path(), &["ctx", cmd, "--grep", "("], "");
    assert_eq!(code, 2);
    assert!(err.contains("invalid --grep regex"), "{err}");
}

#[cfg(unix)]
#[test]
fn ctx_grep_context_flags() {
    let d = tempfile::tempdir().unwrap();
    let run = |flags: &[&str]| {
        let mut args = vec!["ctx", GREP_LINES, "--grep", "^m"];
        args.extend_from_slice(flags);
        let (code, out, err) = acus_in(d.path(), &args, "");
        assert_eq!(code, 0, "{err}");
        out
    };
    // Each group is a match and its neighbours; groups that do not touch are split by `--`.
    assert!(run(&["-A", "1"]).starts_with("m3\nl4\n--\nm8\nl9\n--\nm14\n"));
    assert!(run(&["-B", "1"]).starts_with("l2\nm3\n--\nl7\nm8\n--\nl13\nm14\n"));
    // Touching ranges merge: -C 2 gives lines 1-10 and 12-14.
    assert!(
        run(&["-C", "2"])
            .starts_with("l1\nl2\nm3\nl4\nl5\nl6\nl7\nm8\nl9\nl10\n--\nl12\nl13\nm14\n")
    );
    // -A and -B override -C, whatever the order.
    assert!(
        run(&["-C", "2", "-A", "0"]).starts_with("l1\nl2\nm3\n--\nl6\nl7\nm8\n--\nl12\nl13\nm14\n")
    );
    // The last of repeated flags wins.
    assert!(run(&["-A", "3", "-A", "0"]).starts_with("m3\n--\nm8\n--\nm14\n"));
    // --tail applies after the context is added.
    assert!(run(&["-A", "1", "--tail", "1"]).starts_with("… 4 lines dropped (--tail 1)\nm14\n"));
    // Without --grep the context flags are an error, before the command runs.
    let (code, _, err) = acus_in(d.path(), &["ctx", GREP_LINES, "-C", "1"], "");
    assert_eq!(code, 2);
    assert!(err.contains("-A, -B and -C need --grep"), "{err}");
}

#[test]
fn ctx_shows_code_behind_output_references() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    boom()\n}\n").unwrap();
    let (code, out, _) = acus_in(
        d.path(),
        &["ctx", "echo 'panicked at a.rs:2:5' >&2 && exit 3"],
        "",
    );
    assert_eq!(code, 3);
    assert!(
        out.contains("== exit 3\n== a.rs#f (line 2)\n1\tfn f() {\n2>\t    boom()\n"),
        "{out}"
    );
}

#[cfg(unix)]
#[test]
fn ctx_timeout_kills_the_command_and_keeps_its_output() {
    let t = std::time::Instant::now();
    let (code, out, err) = acus(&["ctx", "echo start; sleep 30", "--timeout", "1"]);
    assert!(t.elapsed().as_secs() < 10, "took {:?}", t.elapsed());
    assert_eq!(code, 124, "{out}{err}");
    assert!(out.lines().any(|l| l == "start"), "{out}");
    assert!(out.contains("killed after 1s (--timeout)"), "{out}");
    assert!(out.contains("== exit 124"), "{out}");
}

#[cfg(unix)]
#[test]
fn ctx_timeout_ends_when_the_child_closed_stdout_but_runs_on() {
    let t = std::time::Instant::now();
    let (code, out, err) = acus(&["ctx", "exec >&-; sleep 30", "--timeout", "1"]);
    assert!(t.elapsed().as_secs() < 10, "took {:?}", t.elapsed());
    assert_eq!(code, 124, "{out}{err}");
}

// On Windows only the shell is killed, not its children; see `capture` in cmd_ctx.
#[cfg(unix)]
#[test]
fn ctx_timeout_kills_grandchildren() {
    let d = tempfile::tempdir().unwrap();
    let pid = d.path().join("pid");
    let cmd = format!("sh -c 'sleep 31 & echo $! > {}; wait'", pid.display());
    let t = std::time::Instant::now();
    let (code, out, err) = acus(&["ctx", &cmd, "--timeout", "1"]);
    assert!(t.elapsed().as_secs() < 10, "took {:?}", t.elapsed());
    assert_eq!(code, 124, "{out}{err}");
    let pid = std::fs::read_to_string(pid).unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "grandchild {pid} survived");
}

#[cfg(unix)]
#[test]
fn ctx_timeout_leaves_a_finished_command_unchanged() {
    let (code, out, err) = acus(&["ctx", "echo hi; exit 3", "--timeout", "10"]);
    assert_eq!(code, 3, "{out}{err}");
    assert!(out.contains("hi") && out.contains("== exit 3"), "{out}");
    assert!(!out.contains("killed"), "{out}");
}

#[test]
fn ctx_log_reads_a_file_through_the_filters() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.rs"), "fn f() {\n    boom()\n}\n").unwrap();
    std::fs::write(
        d.path().join("run.output"),
        "l1\nl2\nerror: boom at a.rs:2:5\nl3\nl4\n",
    )
    .unwrap();
    let (code, out, err) = acus_in(d.path(), &["ctx", "--log", "run.output", "--tail", "1"], "");
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("== log run.output (5 lines)\n"), "{out}");
    assert!(out.contains("error: boom at a.rs:2:5"), "{out}");
    assert!(out.contains("may still be growing"), "{out}"); // just written
}

#[cfg(unix)]
#[test]
fn ctx_cut_clips_each_printed_line() {
    let d = tempfile::tempdir().unwrap();
    let long = "x".repeat(30);
    let cmd = format!("echo short; echo {long}; echo 'error: {long}'; exit 3");
    let (code, out, _) = acus_in(d.path(), &["ctx", &cmd, "--cut", "10"], "");
    assert_eq!(code, 3);
    assert!(out.lines().any(|l| l == "short"), "{out}");
    assert!(out.lines().any(|l| l == "xxxxxxxxxx… (+20 chars)"), "{out}");
    assert!(out.lines().any(|l| l == "error: xxx… (+27 chars)"), "{out}");
    assert!(!out.contains(&long), "{out}");

    let (code, _, _) = acus_in(d.path(), &["ctx", "echo x", "--cut", "0"], "");
    assert_eq!(code, 2);
}

#[cfg(unix)]
#[test]
fn ctx_cut_applies_after_tail() {
    let d = tempfile::tempdir().unwrap();
    let long = "x".repeat(30);
    let cmd = format!("echo {long}; echo 'error: {long}'; exit 3");
    let (code, out, _) = acus_in(d.path(), &["ctx", &cmd, "--tail", "1", "--cut", "10"], "");
    assert_eq!(code, 3);
    assert!(out.lines().any(|l| l == "error: xxx… (+27 chars)"), "{out}");
    assert!(!out.contains(&long), "{out}");
}

#[test]
fn ctx_cut_applies_to_a_log() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("run.output"), "l1\nerror: boom at a.rs:2:5\n").unwrap();
    let (code, out, err) = acus_in(
        d.path(),
        &["ctx", "--log", "run.output", "--tail", "1", "--cut", "10"],
        "",
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("== log run.output (2 lines)\n"), "{out}");
    assert!(out.lines().any(|l| l == "error: boo… (+13 chars)"), "{out}");
}

#[test]
fn ctx_log_with_a_command_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let (code, _, err) = acus_in(d.path(), &["ctx", "echo hi", "--log", "x.output"], "");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--log"), "{err}");
    assert!(
        err.contains("hint: use either a command or --log FILE"),
        "{err}"
    );
}

#[test]
fn ctx_log_missing_file_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let (code, _, err) = acus_in(d.path(), &["ctx", "--log", "nope.output"], "");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("error: nope.output: not found"), "{err}");
}

/// Prints `error: $MSG` and exits with `$CODE`. Every call runs this same command in the same
/// directory, so only the environment differs between runs.
#[cfg(unix)]
const FAILING: &str = "echo \"error: $MSG\"; exit $CODE";

/// Runs `FAILING` in one session whose state lives under `state`.
#[cfg(unix)]
fn failing_run(
    state: &std::path::Path,
    msg: &str,
    code: &str,
    json: bool,
) -> (i32, String, String) {
    let state = state.to_str().unwrap();
    let mut args = vec!["ctx", FAILING];
    if json {
        args.push("--json");
    }
    acus_env(
        &args,
        &[
            ("XDG_STATE_HOME", state),
            ("CLAUDE_CODE_SESSION_ID", "repeat-session"),
            ("MSG", msg),
            ("CODE", code),
        ],
    )
}

#[cfg(unix)]
#[test]
fn ctx_leads_with_the_repeat_line_from_the_third_identical_failure() {
    let state = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let (code, out, err) = failing_run(state.path(), "mismatched", "1", false);
        assert_eq!(code, 1, "{err}");
        assert!(!out.contains("identical failure"), "{out}");
    }
    let (code, out, err) = failing_run(state.path(), "mismatched", "1", false);
    assert_eq!(code, 1, "{err}");
    assert_eq!(
        out.lines().next(),
        Some(
            "… 3rd identical failure of this command in this session, same key line: error: mismatched"
        )
    );
    let (_, out, _) = failing_run(state.path(), "mismatched", "1", false);
    assert!(out.starts_with("… 4th identical failure"), "{out}");
}

#[cfg(unix)]
#[test]
fn ctx_repeat_count_restarts_when_the_failure_changes() {
    let state = tempfile::tempdir().unwrap();
    failing_run(state.path(), "mismatched", "1", false);
    failing_run(state.path(), "mismatched", "1", false);
    let (_, out, _) = failing_run(state.path(), "unresolved", "1", false);
    assert!(!out.contains("identical failure"), "{out}");
    failing_run(state.path(), "unresolved", "1", false);
    let (_, out, _) = failing_run(state.path(), "unresolved", "1", false);
    assert!(out.starts_with("… 3rd identical failure"), "{out}");
}

#[cfg(unix)]
#[test]
fn ctx_success_clears_the_repeat_count() {
    let state = tempfile::tempdir().unwrap();
    failing_run(state.path(), "mismatched", "1", false);
    failing_run(state.path(), "mismatched", "1", false);
    let (code, _, err) = failing_run(state.path(), "mismatched", "0", false);
    assert_eq!(code, 0, "{err}");
    for _ in 0..2 {
        let (_, out, _) = failing_run(state.path(), "mismatched", "1", false);
        assert!(!out.contains("identical failure"), "{out}");
    }
}

#[cfg(unix)]
#[test]
fn ctx_repeat_lead_is_not_cut_by_head() {
    let state = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        failing_run(state.path(), "mismatched", "1", false);
    }
    let state_s = state.path().to_str().unwrap();
    let env = [
        ("XDG_STATE_HOME", state_s),
        ("CLAUDE_CODE_SESSION_ID", "repeat-session"),
        ("MSG", "mismatched"),
        ("CODE", "1"),
    ];
    let (code, out, err) = acus_env(&["ctx", FAILING, "--head", "1"], &env);
    assert_eq!(code, 1, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("… 3rd identical failure"), "{out}");
    assert_eq!(lines[1], "error: mismatched", "{out}");
    assert!(
        lines[2].starts_with("… output cut at --head 1; 1 more line"),
        "{out}"
    );
}

#[cfg(unix)]
#[test]
fn ctx_session_id_cannot_leave_the_ctx_state_dir() {
    let state = tempfile::tempdir().unwrap();
    let state_s = state.path().to_str().unwrap();
    let env = [
        ("XDG_STATE_HOME", state_s),
        ("CLAUDE_CODE_SESSION_ID", "../x"),
        ("MSG", "mismatched"),
        ("CODE", "1"),
    ];
    let (code, _, err) = acus_env(&["ctx", FAILING], &env);
    assert_eq!(code, 1, "{err}");
    let ctx = state.path().join("acus").join("ctx");
    assert!(
        ctx.join("shared.json").is_file(),
        "an invalid session id uses the shared file"
    );
    assert!(
        !state.path().join("acus").join("x.json").exists(),
        "nothing is written outside the ctx state dir"
    );
}

#[cfg(unix)]
#[test]
fn ctx_ignores_a_state_dir_it_cannot_write() {
    let d = tempfile::tempdir().unwrap();
    let blocked = d.path().join("state");
    std::fs::write(&blocked, "not a directory").unwrap();
    for _ in 0..3 {
        let (code, out, err) = failing_run(&blocked, "mismatched", "1", false);
        assert_eq!(code, 1, "{err}");
        assert_eq!(err, "");
        assert!(out.starts_with("error: mismatched"), "{out}");
        assert!(!out.contains("identical failure"), "{out}");
    }
}

#[cfg(unix)]
#[test]
fn ctx_json_adds_repeat_from_the_third_identical_failure() {
    let state = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let (_, out, _) = failing_run(state.path(), "mismatched", "1", true);
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(doc.get("repeat").is_none(), "{out}");
    }
    let (code, out, err) = failing_run(state.path(), "mismatched", "1", true);
    assert_eq!(code, 1, "{err}");
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["exit"], 1);
    assert_eq!(
        doc["repeat"],
        serde_json::json!({"count": 3, "key": "error: mismatched"})
    );
}

#[test]
fn ctx_log_strips_github_actions_prefixes() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("src")).unwrap();
    std::fs::write(
        d.path().join("src/lib.rs"),
        "fn a() {}\nfn b() {}\nfn c() { x }\n",
    )
    .unwrap();
    let log = "build\tRun cargo test\t2026-01-05T10:00:00.1234567Z    Compiling demo v0.1.0\n\
               build\tRun cargo test\t2026-01-05T10:00:01.0000000Z error[E0425]: cannot find value `x` in this scope\n\
               build\tRun cargo test\t2026-01-05T10:00:01.0000001Z  --> src/lib.rs:3:10\n\
               build\tRun cargo test\t2026-01-05T10:00:02.0000000Z ##[endgroup]\n";
    std::fs::write(d.path().join("job.log"), log).unwrap();
    let (code, out, err) = acus_in(d.path(), &["ctx", "--log", "job.log"], "");
    assert_eq!(code, 0, "{err}");
    let out = out
        .lines()
        .filter(|l| !l.contains("may still be growing"))
        .collect::<Vec<_>>()
        .join("\n");
    let _dropguard = snapshot_settings().bind_to_scope();
    insta::assert_snapshot!("cli__ctx_github_prefixes", out);
}

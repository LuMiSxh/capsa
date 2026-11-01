// SPDX-License-Identifier: MPL-2.0
use crate::common::*;

// Bash calls are rewritten only with a Bash-like grammar (`guard-bash` or `guard-zsh`).
#[cfg(any(feature = "guard-bash", feature = "guard-zsh"))]
#[test]
fn guard_answers_hook_calls() {
    use std::io::Write;
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let guard = |input: serde_json::Value| {
        let mut child = bin()
            .arg("guard")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        write!(child.stdin.take().unwrap(), "{input}").unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        String::from_utf8(out.stdout).unwrap()
    };
    let out = guard(serde_json::json!({
        "tool_name": "Grep",
        "cwd": dir,
        "tool_input": {"pattern": "needle", "path": dir.join("src"), "output_mode": "content"},
    }));
    assert!(out.contains(r#""permissionDecision":"deny""#), "{out}");
    assert!(out.contains("acus find 'needle' 'src'"), "{out}");
    let out = guard(serde_json::json!({
        "tool_name": "PowerShell",
        "tool_input": {"command": "Get-Content src\\lib.rs"},
    }));
    assert!(out.contains("acus-skip"), "{out}");
    let out = guard(serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}));
    assert_eq!(out, "");
    let out = guard(serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {"command": "cd app && ls; cat settings.json; grep -n foo src/a.rs && echo ok"},
    }));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let o = &v["hookSpecificOutput"];
    assert_eq!(o["permissionDecision"], "allow", "{out}");
    assert_eq!(
        o["updatedInput"]["command"],
        "cd app && ls; cat settings.json; acus find 'foo' 'src/a.rs' && echo ok",
        "{out}"
    );
    assert!(o.get("permissionDecisionReason").is_none(), "{out}");
    assert!(
        o["additionalContext"]
            .as_str()
            .unwrap()
            .contains("instead of `grep -n foo src/a.rs`"),
        "{out}"
    );
    let out = guard(serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {"command": "tail -n 5 src/a.rs; find src -name '*.rs'"},
    }));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["hookSpecificOutput"]["updatedInput"]["command"],
        "acus show 'src/a.rs:-5'; acus files 'src' -g '*.rs'",
        "{out}"
    );
    // Other tool_input fields survive; a build pipe is rewritten in every mode, with no decision.
    let build = serde_json::json!({
        "tool_name": "Bash",
        "permission_mode": "default",
        "tool_input": {"command": "cargo test 2>&1 | tail -3", "timeout": 5000},
    });
    let v: serde_json::Value = serde_json::from_str(&guard(build)).unwrap();
    let o = &v["hookSpecificOutput"];
    assert!(o.get("permissionDecision").is_none(), "{v}");
    assert_eq!(
        o["updatedInput"]["command"],
        "acus ctx 'cargo test' --tail 3"
    );
    assert_eq!(o["updatedInput"]["timeout"], 5000);
    // In-place edits have no exact rewrite.
    let out = guard(serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {"command": "cd app && cat a.rs; sed -i 's/a/b/' b.rs"},
    }));
    assert!(out.contains(r#""permissionDecision":"deny""#), "{out}");
    assert!(out.contains("Nothing ran"), "{out}");
}

#[test]
fn guard_never_fails_on_malformed_input() {
    for input in [
        "",
        "not json",
        "[]",
        "null",
        r#"{"tool_name": "Bash"}"#,
        r#"{"tool_name": "Bash", "tool_input": null}"#,
        r#"{"tool_name": "Bash", "tool_input": {"command": 5}}"#,
        r#"{"tool_name": "Bash", "tool_input": {"command": "grep -e '"}}"#,
        r#"{"tool_name": "PowerShell", "tool_input": {"command": "findstr /é é"}}"#,
        r#"{"tool_name": "Read", "tool_input": {"file_path": 7}}"#,
        r#"{"tool_name": "Grep", "tool_input": {"pattern": 7}}"#,
        r#"{"tool_name": 3}"#,
    ] {
        let (code, out, _) = acus_in(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
            &["guard"],
            input,
        );
        // Exit 0 with nothing to say, or with a verdict for what could be read.
        assert_eq!(code, 0, "{input}");
        assert!(
            out.is_empty() || out.starts_with(r#"{"hookSpecificOutput""#),
            "{input}: {out}"
        );
    }
}

#[cfg(not(feature = "cmd-decide"))]
#[test]
fn decide_without_feature_has_hint() {
    let (code, _, err) = acus(&["decide", "ok?", "--state", "x"]);
    assert_eq!(code, 2);
    assert!(err.contains("--features cmd-decide"), "{err}");
}

#[cfg(feature = "cmd-decide")]
#[test]
fn decide_without_endpoint_has_hint() {
    let (code, _, err) = acus(&["decide", "ok?", "--state", "x"]);
    assert_eq!(code, 2);
    assert!(err.contains("ACUS_DECIDE_URL"), "{err}");
}

#[cfg(feature = "cmd-decide")]
#[test]
fn decide_without_key_warns_and_skips() {
    let env = [
        ("ACUS_DECIDE_URL", "http://127.0.0.1:9"),
        ("ACUS_DECIDE_API_KEY_ENV", "ACUS_TEST_UNSET_KEY"),
    ];
    let (code, _, err) = acus_env(&["decide", "ok?", "--state", "x"], &env);
    assert_eq!(code, 2);
    assert!(
        err.contains("warning: no API key in $ACUS_TEST_UNSET_KEY"),
        "{err}"
    );
}

#[cfg(any(feature = "guard-bash", feature = "guard-zsh"))]
#[test]
fn guard_rewrite_quotes_every_copied_argument() {
    use std::io::Write;
    let run = |command: &str| {
        let mut child = bin()
            .arg("guard")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let input = serde_json::json!({"tool_name": "Bash", "tool_input": {"command": command}});
        write!(child.stdin.take().unwrap(), "{input}").unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        let out = String::from_utf8(out.stdout).unwrap();
        serde_json::from_str::<serde_json::Value>(&out)
            .ok()
            .and_then(|v| {
                v["hookSpecificOutput"]["updatedInput"]["command"]
                    .as_str()
                    .map(str::to_owned)
            })
    };
    for (input, want) in [
        (
            "cat 'a;touch pwn;.rs' b.rs",
            "acus show 'a;touch pwn;.rs' 'b.rs'",
        ),
        (
            "rg -t 'py;touch pwn' foo src",
            "acus find 'foo' 'src' -t 'py;touch pwn'",
        ),
        ("cat 'my file.rs' b.rs", "acus show 'my file.rs' 'b.rs'"),
        ("cat 'a&b.rs' b.rs", "acus show 'a&b.rs' 'b.rs'"),
    ] {
        assert_eq!(run(input).as_deref(), Some(want), "{input}");
    }
}

/// Runs `acus ARGS` in the fixture project with `ACUS_CONFIG` at `config` and `input` on stdin.
#[expect(clippy::needless_pass_by_value, reason = "callers pass a JSON literal")]
fn run_hook(config: &std::path::Path, args: &[&str], input: serde_json::Value) -> String {
    let proj = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    let cfg = config.display().to_string();
    let (code, out, err) = acus_in_env(
        proj,
        args,
        &input.to_string(),
        &[("ACUS_CONFIG", cfg.as_str())],
    );
    assert_eq!(code, 0, "{err}");
    out
}

fn bash_call(session: &str, command: &str) -> serde_json::Value {
    serde_json::json!({"session_id": session, "tool_name": "Bash", "tool_input": {"command": command}})
}

/// Pure hints show on the 1st, 11th… call of a rule per session (`hint_every` = 10); the other
/// calls print nothing. Rewrites and refusals show on every call.
#[test]
fn guard_throttles_pure_hints_per_session() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[guard]\nhint_every = 10\n").unwrap();
    let shown: Vec<usize> = (1..=12)
        .filter(|_| !run_hook(&config, &["guard"], bash_call("s1", "git status")).is_empty())
        .collect();
    assert_eq!(shown, [1, 11]);
    // Another session counts on its own.
    assert!(!run_hook(&config, &["guard"], bash_call("s2", "git status")).is_empty());
    for _ in 0..12 {
        let rewrite = run_hook(&config, &["guard"], bash_call("s1", "tail -n 5 src/a.rs"));
        assert!(rewrite.contains("updatedInput"), "{rewrite}");
        let deny = run_hook(
            &config,
            &["guard"],
            bash_call("s1", "cd app && cat a.rs; sed -i 's/a/b/' b.rs"),
        );
        assert!(deny.contains(r#""permissionDecision":"deny""#), "{deny}");
    }
}

/// Parallel hook processes of one session count every call and show the hint exactly on calls 1
/// and 11.
#[test]
fn concurrent_guard_calls_count_exactly() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[guard]\nhint_every = 10\n").unwrap();
    let (next, shown) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                while next.fetch_add(1, Ordering::SeqCst) < 20 {
                    let out = run_hook(&config, &["guard"], bash_call("par", "git status"));
                    if !out.is_empty() {
                        shown.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    });
    // A call that gets the lock counts and shows a hint iff the old count is a multiple of 10; one
    // that times out shows a hint and counts nothing. A lost update would break this equation.
    let state = dir.path().join("guard-hints");
    let text = std::fs::read_to_string(state.join("par")).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    let last: usize = text.trim_end().rsplit(' ').next().unwrap().parse().unwrap();
    assert!((1..=20).contains(&last), "{text}");
    assert_eq!(
        shown.load(Ordering::SeqCst),
        last.div_ceil(10) + (20 - last),
        "{text}"
    );
    assert!(!state.join("par.lock").exists());
}

/// The SessionStart hook forgets a session's counts after a compaction or `/clear`, so the next
/// hint shows again. After a startup the count goes on.
#[test]
fn session_start_after_compaction_resets_hints() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[guard]\nhint_every = 10\n").unwrap();
    let hint = || run_hook(&config, &["guard"], bash_call("s1", "git status"));
    let start = |source: &str| {
        run_hook(
            &config,
            &["self", "skill"],
            serde_json::json!({"session_id": "s1", "source": source}),
        )
    };
    assert!(!hint().is_empty());
    assert!(hint().is_empty());
    assert!(!start("startup").is_empty());
    assert!(hint().is_empty());
    assert!(!start("compact").is_empty());
    assert!(!hint().is_empty());
    assert!(hint().is_empty());
}

#[test]
fn skill_prints_without_valid_hook_input() {
    let proj = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proj"));
    for input in ["", "not json"] {
        let (code, out, _) = acus_in_env(proj, &["self", "skill"], input, &[]);
        assert_eq!(code, 0);
        assert!(!out.is_empty(), "{input}");
    }
}

/// `hint_every` = 1 shows every hint, and 0 counts as 1.
#[test]
fn hint_every_one_shows_every_hint() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    for every in ["1", "0"] {
        std::fs::write(&config, format!("[guard]\nhint_every = {every}\n")).unwrap();
        for _ in 0..12 {
            let out = run_hook(&config, &["guard"], bash_call("s1", "git status"));
            assert!(!out.is_empty(), "hint_every = {every}");
        }
    }
}

/// A state directory that cannot be used (here a file with its name) fails open: every hint
/// shows.
#[test]
fn unusable_hint_state_still_shows_hints() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[guard]\nhint_every = 10\n").unwrap();
    std::fs::write(dir.path().join("guard-hints"), "not a directory").unwrap();
    for _ in 0..3 {
        let out = run_hook(&config, &["guard"], bash_call("s1", "git status"));
        assert!(!out.is_empty());
    }
}

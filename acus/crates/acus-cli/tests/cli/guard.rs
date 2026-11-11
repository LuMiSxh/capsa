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
            .arg("hook")
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
            &["hook"],
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

#[cfg(any(feature = "guard-bash", feature = "guard-zsh"))]
#[test]
fn guard_rewrite_quotes_every_copied_argument() {
    use std::io::Write;
    let run = |command: &str| {
        let mut child = bin()
            .arg("hook")
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
        .filter(|_| !run_hook(&config, &["hook"], bash_call("s1", "git status")).is_empty())
        .collect();
    assert_eq!(shown, [1, 11]);
    // Another session counts on its own.
    assert!(!run_hook(&config, &["hook"], bash_call("s2", "git status")).is_empty());
    for _ in 0..12 {
        let rewrite = run_hook(&config, &["hook"], bash_call("s1", "tail -n 5 src/a.rs"));
        assert!(rewrite.contains("updatedInput"), "{rewrite}");
        let deny = run_hook(
            &config,
            &["hook"],
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
                    let out = run_hook(&config, &["hook"], bash_call("par", "git status"));
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
    let lock = capsa_core::lock::exclusive(&state.join("par.lock"), std::time::Duration::ZERO);
    assert!(lock.unwrap().is_some(), "every call released the lock");
}

/// The SessionStart hook forgets a session's counts after a compaction or `/clear`, so the next
/// hint shows again. After a startup the count goes on.
#[test]
fn session_start_after_compaction_resets_hints() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[guard]\nhint_every = 10\n").unwrap();
    let hint = || run_hook(&config, &["hook"], bash_call("s1", "git status"));
    let start = |source: &str| {
        run_hook(
            &config,
            &["hook"],
            serde_json::json!({
                "hook_event_name": "SessionStart",
                "session_id": "s1",
                "source": source,
            }),
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
fn skill_prints_without_stdin() {
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
            let out = run_hook(&config, &["hook"], bash_call("s1", "git status"));
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
        let out = run_hook(&config, &["hook"], bash_call("s1", "git status"));
        assert!(!out.is_empty());
    }
}

/// A config that does not parse must not stop the guard: it answers with the defaults, names the
/// problem once per session, and still rewrites.
#[test]
fn guard_answers_with_defaults_under_a_broken_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("acus.toml");
    std::fs::write(&config, "[guard]\nhint_every = \"ten\"\n").unwrap();
    let first = run_hook(&config, &["hook"], bash_call("c1", "ls"));
    assert!(
        first.contains("acus config error") && first.contains("acus.toml"),
        "{first}"
    );
    let second = run_hook(&config, &["hook"], bash_call("c1", "ls"));
    assert!(second.is_empty(), "{second}");
    let rewrite = run_hook(&config, &["hook"], bash_call("c2", "tail -n 5 src/a.rs"));
    assert!(rewrite.contains("updatedInput"), "{rewrite}");
    assert!(rewrite.contains("acus config error"), "{rewrite}");
}

/// `acus hook` dispatches on `hook_event_name`: `SessionStart` prints the skill, an event the
/// guard does not handle prints nothing, and no event (Codex) still runs the guard.
#[test]
fn hook_dispatches_on_the_event() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let session = run_hook(
        &config,
        &["hook"],
        serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": "s1",
            "source": "startup",
        }),
    );
    assert!(session.starts_with("# acus"), "{session}");
    let other = run_hook(
        &config,
        &["hook"],
        serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Grep",
            "tool_input": {"pattern": "needle"},
        }),
    );
    assert!(other.is_empty(), "{other}");
    let codex = run_hook(
        &config,
        &["hook"],
        serde_json::json!({
            "turn_id": "t1",
            "tool_name": "Bash",
            "tool_input": {"command": "grep -n foo src/a.rs"},
        }),
    );
    assert!(codex.contains(r#""permissionDecision":"deny""#), "{codex}");
    assert!(!codex.contains("updatedInput"), "{codex}");
}

/// A SubagentStart gets the core skill as `additionalContext`, since a subagent does not see
/// SessionStart output.
#[test]
fn hook_gives_a_subagent_the_core_skill() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let out = run_hook(
        &config,
        &["hook"],
        serde_json::json!({
            "hook_event_name": "SubagentStart",
            "agent_type": "Explore",
        }),
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let hook = &v["hookSpecificOutput"];
    assert_eq!(hook["hookEventName"], "SubagentStart");
    assert_eq!(
        hook["additionalContext"].as_str().unwrap(),
        include_str!("../../../../skill/CORE.md")
    );
}

/// Claude Code and Codex parse the hook's bytes, so each reply shape is pinned as the exact line
/// `acus hook` prints, field order included.
#[cfg(any(feature = "guard-bash", feature = "guard-zsh"))]
#[test]
fn hook_replies_keep_their_wire_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[guard]\nhint_every = 10\n").unwrap();
    let reply = |input: serde_json::Value| run_hook(&config, &["hook"], input);
    assert_eq!(
        reply(serde_json::json!({
            "session_id": "r1",
            "tool_name": "Bash",
            "tool_input": {"command": "cd app && ls; cat settings.json; grep -n foo src/a.rs && echo ok"},
        })),
        format!(
            "{}\n",
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"cd app && ls; cat settings.json; acus find 'foo' 'src/a.rs' && echo ok"},"additionalContext":"acus guard: ran `acus find 'foo' 'src/a.rs'` instead of `grep -n foo src/a.rs`. The command was rewritten, not refused; use acus directly next time.","permissionDecision":"allow"}}"#
        )
    );
    assert_eq!(
        reply(serde_json::json!({
            "tool_name": "Bash",
            "permission_mode": "default",
            "tool_input": {"command": "cargo test 2>&1 | tail -3", "timeout": 5000},
        })),
        format!(
            "{}\n",
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"acus ctx 'cargo test' --tail 3","timeout":5000},"additionalContext":"acus guard: ran `acus ctx 'cargo test' --tail 3` instead of `cargo test 2>&1 | tail -3`. The command was rewritten, not refused; use acus directly next time."}}"#
        )
    );
    assert_eq!(
        reply(serde_json::json!({
            "turn_id": "t1",
            "tool_name": "Bash",
            "tool_input": {"command": "grep -n foo src/a.rs"},
        })),
        format!(
            "{}\n",
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Use acus instead of grep: `acus find 'PAT' [PATH…]` (`--block` adds the enclosing code, `-l` lists files, `-w`, `-t TYPE`, `-g GLOB`, `-u --hidden` include ignored and hidden files). Only if acus cannot do this, rerun with a `command ` prefix."}}"#
        )
    );
    assert_eq!(
        reply(bash_call("h1", "git status")),
        format!(
            "{}\n",
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"acus hint: `acus diff` lists uncommitted changes per file and function with line counts, untracked files included; `-p` adds the changed lines and `--staged` limits it to the index."}}"#
        )
    );
    assert_eq!(
        reply(serde_json::json!({"hook_event_name": "SubagentStart", "agent_type": "Explore"})),
        format!(
            "{}\n",
            serde_json::json!({"hookSpecificOutput": {
                "hookEventName": "SubagentStart",
                "additionalContext": include_str!("../../../../skill/CORE.md"),
            }})
        )
    );
    let bad = dir.path().join("bad.toml");
    std::fs::write(&bad, "[guard]\nhint_every = \"ten\"\n").unwrap();
    let note = run_hook(&bad, &["hook"], bash_call("c1", "ls"));
    let want = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"acus config error: invalid config PATH: TOML parse error at line 2, column 14 | 2 | hint_every = \"ten\" | ^^^^^ invalid type: string \"ten\", expected usize; guard runs with defaults; fix PATH"}}"#;
    assert_eq!(
        note,
        format!("{}\n", want.replace("PATH", &bad.display().to_string()))
    );
}

// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn context_reads_the_session_of_this_directory() {
    let home = Scratch::new("cli-context");
    let cwd = home.path().join("work");
    std::fs::create_dir_all(&cwd).expect("create the working directory");
    let id = claude_session(&home, &cwd);

    let out = census(home.path(), &cwd, &["context"], "", &[]);
    assert_eq!(out.status.code(), Some(0));
    let shown = text(&out.stdout);
    assert!(shown.contains("60k of 400k (15%), 340k left"), "{shown}");
    assert!(
        shown.contains("400k from autoCompactWindow in settings.json"),
        "{shown}"
    );
    assert!(shown.contains("claude-opus-5-5"), "{shown}");
    assert!(
        shown.contains(&format!("{id} (most recent transcript")),
        "{shown}"
    );

    let json = census(home.path(), &cwd, &["--json", "context"], "", &[]);
    let shown = text(&json.stdout);
    assert!(
        shown.contains("\"tokens\":60000") && shown.contains("\"picked\":\"recent\""),
        "{shown}"
    );

    let by_env = census(
        home.path(),
        &cwd,
        &["context"],
        "",
        &[("CLAUDE_SESSION_ID", id)],
    );
    assert!(text(&by_env.stdout).contains("(CLAUDE_SESSION_ID)"));

    home.write(
        "claude/projects/app/s2.jsonl",
        &jsonl(&[
            json!({"type": "user", "timestamp": "2026-10-14T08:00:00.000Z",
                "message": {"role": "user", "content": "go"}}),
            json!({"type": "assistant", "timestamp": "2026-10-14T08:00:10.000Z",
                "message": {"id": "m9", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 1000, "output_tokens": 10}}}),
        ]),
    );
    let newest = census(
        home.path(),
        &cwd,
        &["context"],
        "",
        &[("CLAUDE_CODE_SESSION_ID", id), ("CLAUDE_SESSION_ID", "s2")],
    );
    assert!(
        text(&newest.stdout).contains(&format!("{id} (CLAUDE_CODE_SESSION_ID)")),
        "{}",
        text(&newest.stdout)
    );
    let empty_wins = census(
        home.path(),
        &cwd,
        &["context"],
        "",
        &[("CLAUDE_CODE_SESSION_ID", ""), ("CLAUDE_SESSION_ID", "s2")],
    );
    assert!(
        text(&empty_wins.stdout).contains("s2 (CLAUDE_SESSION_ID)"),
        "{}",
        text(&empty_wins.stdout)
    );
}

#[test]
fn session_summarizes_the_session_of_this_directory() {
    let home = Scratch::new("cli-session");
    let cwd = home.path().join("work");
    std::fs::create_dir_all(&cwd).expect("create the working directory");
    claude_session(&home, &cwd);
    let out = census(home.path(), &cwd, &["session"], "", &[]);
    assert_eq!(out.status.code(), Some(0));
    let shown = text(&out.stdout);
    assert!(shown.contains("2 (1 subagents)"), "{shown}");
    assert!(shown.contains("worker 1"), "{shown}");
    assert!(shown.contains("compactions: 0"), "{shown}");
    assert!(shown.contains("60k of 400k"), "{shown}");
}

#[test]
fn context_and_session_name_an_unknown_session() {
    let home = Scratch::new("cli-unknown");
    let cwd = home.path().to_path_buf();
    let out = census(
        home.path(),
        &cwd,
        &["session", "--session", "nope"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("no transcript for session nope"));
}

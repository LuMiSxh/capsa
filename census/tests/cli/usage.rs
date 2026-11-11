// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn misses_needs_tools() {
    let home = Scratch::new("cli-misses-requires");
    let out = census(home.path(), home.path(), &["usage", "--misses"], "", &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("--tools"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn usage_misses_lists_the_commands_that_bypass_acus() {
    let home = Scratch::new("cli-misses");
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            json!({"type": "assistant", "timestamp": "2026-10-14T08:00:10.000Z",
                "message": {"id": "m1", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 10, "output_tokens": 1},
                    "content": [
                        {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "git diff --cached"}},
                        {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "git diff --cached HEAD~1"}},
                        {"type": "tool_use", "id": "t3", "name": "Read", "input": {"file_path": "a"}},
                    ]}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "0123456789"},
                {"type": "tool_result", "tool_use_id": "t2", "content": "abc"},
                {"type": "tool_result", "tool_use_id": "t3", "content": "xy"},
            ]}}),
        ]),
    );
    let out = census(
        home.path(),
        home.path(),
        &["usage", "--tools", "--misses", "--top", "2"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let output = text(&out.stdout);
    assert!(output.contains("commands that bypass acus"), "{output}");
    assert!(output.contains("git diff"), "{output}");
    assert!(output.contains("Read"), "{output}");

    let out = census(
        home.path(),
        home.path(),
        &["--json", "usage", "--tools", "--misses", "--top", "2"],
        "",
        &[],
    );
    let doc: Value = serde_json::from_slice(&out.stdout).expect("census prints JSON");
    assert_eq!(
        doc["misses"],
        json!([
            {"category": "diff", "command": "git diff --cached", "calls": 2,
             "intercepted": 0, "result_chars": 13},
            {"category": "read", "command": "Read", "calls": 1,
             "intercepted": 0, "result_chars": 2},
        ])
    );
    assert!(doc["tools"].as_array().is_some_and(|t| !t.is_empty()));
}

#[test]
fn usage_tools_reports_percentiles_of_sorted_result_sizes() {
    let home = Scratch::new("cli-tools-percentiles");
    let sizes = [5000, 100, 400, 200, 300];
    let mut calls = vec![];
    let mut results = vec![];
    for (i, n) in sizes.iter().enumerate() {
        calls.push(
            json!({"type": "tool_use", "id": format!("t{i}"), "name": "Bash",
            "input": {"command": format!("cat f{i}")}}),
        );
        results.push(
            json!({"type": "tool_result", "tool_use_id": format!("t{i}"),
            "content": "x".repeat(*n)}),
        );
    }
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            json!({"type": "assistant", "timestamp": "2026-10-14T08:00:10.000Z",
                "message": {"id": "m1", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 10, "output_tokens": 1},
                    "content": calls}}),
            json!({"type": "user", "message": {"role": "user", "content": results}}),
        ]),
    );
    let out = census(home.path(), home.path(), &["usage", "--tools"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let output = text(&out.stdout);
    assert!(
        output.contains("explore/read, 5, 0, 5, 100%, 6k, 100%, 300, 5k, 5k"),
        "{output}"
    );

    let out = census(
        home.path(),
        home.path(),
        &["--json", "usage", "--tools"],
        "",
        &[],
    );
    let doc: Value = serde_json::from_slice(&out.stdout).expect("census prints JSON");
    let read = doc["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["category"] == "read")
        .expect("the read row");
    assert_eq!(
        (
            read["result_p50"].as_u64(),
            read["result_p90"].as_u64(),
            read["result_max"].as_u64(),
        ),
        (Some(300), Some(5000), Some(5000))
    );
}

#[test]
fn usage_misses_excludes_acus_shaped_commands() {
    let home = Scratch::new("cli-misses-acus");
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            json!({"type": "assistant", "timestamp": "2026-10-14T08:00:10.000Z",
                "message": {"id": "m1", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 10, "output_tokens": 1},
                    "content": [
                        {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "acus find 'x'"}},
                        {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "acus run <<'EOF'\nfind 'x'\nEOF"}},
                        {"type": "tool_use", "id": "t3", "name": "Bash", "input": {"command": "acus show a.rs:1-5"}},
                        {"type": "tool_use", "id": "t4", "name": "Bash", "input": {"command": "cat f"}},
                        {"type": "tool_use", "id": "t5", "name": "Bash", "input": {"command": "sed -n 1,5p f"}},
                    ]}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "aaaa"},
                {"type": "tool_result", "tool_use_id": "t2", "content": "bbbb"},
                {"type": "tool_result", "tool_use_id": "t3", "content": "cccc"},
                {"type": "tool_result", "tool_use_id": "t4", "content": "dddd"},
                {"type": "tool_result", "tool_use_id": "t5", "content": "eeee"},
            ]}}),
        ]),
    );
    let out = census(
        home.path(),
        home.path(),
        &["--json", "usage", "--tools", "--misses"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let doc: Value = serde_json::from_slice(&out.stdout).expect("census prints JSON");
    assert_eq!(
        doc["misses"],
        json!([
            {"category": "read", "command": "cat", "calls": 1,
             "intercepted": 0, "result_chars": 4},
            {"category": "read", "command": "sed -n", "calls": 1,
             "intercepted": 0, "result_chars": 4},
        ])
    );
    assert!(
        doc["misses"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| !m["command"].as_str().unwrap_or("").starts_with("acus")),
        "{}",
        doc["misses"]
    );
    let categories: Vec<_> = doc["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["category"].as_str())
        .collect();
    for category in ["acus:find", "acus:run", "acus:show"] {
        assert!(categories.contains(&category), "{categories:?}");
    }
}

#[test]
fn usage_misses_counts_only_calls_the_guard_let_run() {
    let home = Scratch::new("cli-misses-intercepted");
    let heredoc = "cat > src/b.rs <<'EOF'\nfn bob() {}\nEOF";
    home.write(
        "claude/projects/app/s1.jsonl",
        &jsonl(&[
            json!({"type": "assistant", "timestamp": "2026-10-14T08:00:10.000Z",
                "message": {"id": "m1", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 10, "output_tokens": 1},
                    "content": [
                        {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cat src/a.rs"}},
                        {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "git log -1"}},
                        {"type": "tool_use", "id": "t3", "name": "Bash", "input": {"command": "git diff --cached HEAD~1 | head"}},
                        {"type": "tool_use", "id": "t4", "name": "Bash", "input": {"command": "git diff --cached | head"}},
                        {"type": "tool_use", "id": "t5", "name": "Bash", "input": {"command": heredoc}},
                        {"type": "tool_use", "id": "t6", "name": "Bash", "input": {"command": "grep foo"}},
                    ]}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": true,
                 "content": "PreToolUse:Bash hook error: Nothing ran: `cat src/a.rs` is refused. Replace it with `acus show src/a.rs` and send the command again. Use acus instead of cat."},
                {"type": "tool_result", "tool_use_id": "t2", "content": "log output"},
                {"type": "tool_result", "tool_use_id": "t3", "content": "rewritten output"},
                {"type": "tool_result", "tool_use_id": "t4", "content": "abcde"},
                {"type": "tool_result", "tool_use_id": "t5", "content": "xy"},
                {"type": "tool_result", "tool_use_id": "t6", "is_error": true,
                 "content": "PreToolUse:Bash hook error: policy blocks grep"},
            ]}}),
            json!({"type": "attachment", "attachment": {
                "type": "hook_additional_context",
                "content": ["acus guard: ran `acus log -n 1` instead of `git log -1`. The command was rewritten, not refused; use acus directly next time."],
                "hookName": "PreToolUse:Bash", "toolUseID": "t2", "hookEvent": "PreToolUse"}}),
            json!({"type": "attachment", "attachment": {
                "type": "hook_additional_context",
                "content": ["acus guard: ran `acus diff --cached --head 20` instead of `git diff --cached HEAD~1 | head`. The command was rewritten, not refused; use acus directly next time."],
                "hookName": "PreToolUse:Bash", "toolUseID": "t3", "hookEvent": "PreToolUse"}}),
        ]),
    );
    let out = census(
        home.path(),
        home.path(),
        &["usage", "--tools", "--misses"],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let output = text(&out.stdout);
    assert!(output.contains("intercepted"), "{output}");
    assert!(
        output.contains("diff, git diff --cached | head, 1, 1, 5"),
        "{output}"
    );
    assert!(output.contains("edit-shell, cat >, 1, 0, 2"), "{output}");
    assert!(output.contains("search, grep, 1, 0, 46"), "{output}");

    let out = census(
        home.path(),
        home.path(),
        &["--json", "usage", "--tools", "--misses"],
        "",
        &[],
    );
    let doc: Value = serde_json::from_slice(&out.stdout).expect("census prints JSON");
    assert_eq!(
        doc["misses"],
        json!([
            {"category": "diff", "command": "git diff --cached | head", "calls": 1,
             "intercepted": 1, "result_chars": 5},
            {"category": "edit-shell", "command": "cat >", "calls": 1,
             "intercepted": 0, "result_chars": 2},
            {"category": "search", "command": "grep", "calls": 1,
             "intercepted": 0, "result_chars": 46},
        ])
    );
}

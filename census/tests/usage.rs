// SPDX-License-Identifier: MPL-2.0
use census::usage::{UsageOpts, report};
use std::fs;

const CLAUDE: &str = r#"{"type":"user","cwd":"/x/Proj","message":{"role":"user","content":"do it"}}
{"type":"assistant","cwd":"/x/Proj","message":{"id":"m1","model":"opus","usage":{"input_tokens":10,"cache_read_input_tokens":1000,"cache_creation":{"ephemeral_1h_input_tokens":100,"ephemeral_5m_input_tokens":0},"output_tokens":20},"content":[{"type":"tool_use","id":"t1","name":"Grep","input":{"pattern":"x"}}]}}
{"type":"assistant","cwd":"/x/Proj","message":{"id":"m1","model":"opus","usage":{"input_tokens":10,"cache_read_input_tokens":1000,"cache_creation":{"ephemeral_1h_input_tokens":100,"ephemeral_5m_input_tokens":0},"output_tokens":20},"content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"a"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"0123456789"}]}}
{"type":"assistant","message":{"id":"m2","model":"opus","usage":{"input_tokens":0,"cache_read_input_tokens":2000,"output_tokens":5},"content":[{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"sed -n 1,5p a"}}]}}
{"type":"assistant","message":{"id":"m3","model":"opus","usage":{"input_tokens":0,"cache_read_input_tokens":3000,"output_tokens":5},"content":[{"type":"tool_use","id":"t4","name":"Edit","input":{}}]}}
{"type":"system","subtype":"compact_boundary"}
"#;

const CODEX: &str = r#"{"type":"session_meta","payload":{"cwd":"/y/Other"}}
{"type":"turn_context","payload":{"model":"gpt-x"}}
{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}
{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"rg foo\"}","call_id":"c1"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":80,"cache_write_input_tokens":0,"output_tokens":10}}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":80,"cache_write_input_tokens":0,"output_tokens":10}}}}
{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"abc"}}
"#;

#[test]
fn aggregates_both_agents() {
    let d = tempfile::tempdir().unwrap();
    let claude = d.path().join("claude");
    let codex = d.path().join("codex");
    fs::create_dir_all(claude.join("-x-Proj/s1/subagents")).unwrap();
    fs::create_dir_all(codex.join("2026/10/03")).unwrap();
    fs::write(claude.join("-x-Proj/s1.jsonl"), CLAUDE).unwrap();
    fs::write(codex.join("2026/10/03/rollout-a.jsonl"), CODEX).unwrap();
    let r = report(&UsageOpts {
        claude_root: Some(claude),
        codex_root: Some(codex),
        ..Default::default()
    });

    assert_eq!(r.prompts, 2);
    let s: Vec<_> = r
        .sessions
        .iter()
        .map(|s| (s.agent, s.project.as_str(), s.tokens.requests))
        .collect();
    assert_eq!(
        s,
        [("claude", "Proj", 3), ("codex", "Other", 1)],
        "dedup by message id / total"
    );
    let c = &r.sessions[0];
    assert_eq!(
        (c.tokens.cache_read, c.tokens.cache_write_1h, c.compactions),
        (6000, 100, 1)
    );
    assert_eq!(c.max_context, 3000);
    let x = &r.sessions[1].tokens;
    assert_eq!((x.input, x.cache_read, x.output), (20, 80, 10));

    let grep = r.tools.iter().find(|t| t.name == "Grep").unwrap();
    // Result size counts the text, not the JSON string around it.
    assert_eq!((grep.calls, grep.result_chars), (1, 10));
    // Grep, Read, sed form one chain of 3; Edit ends it.
    assert_eq!(
        (r.chains.count, r.chains.calls_in_chains, r.chains.longest),
        (1, 3, 3)
    );
    assert_eq!(r.chains.tool_calls, 5);
}

#[test]
fn project_filter() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("p")).unwrap();
    fs::write(d.path().join("p/s.jsonl"), CLAUDE).unwrap();
    let r = report(&UsageOpts {
        claude_root: Some(d.path().into()),
        codex_root: Some(d.path().join("none")),
        project: Some("nomatch".into()),
        ..Default::default()
    });
    assert!(r.sessions.is_empty());
}

#[test]
fn agents_one_row_per_role() {
    let line = |id: &str, model: &str, input: u64| {
        format!(
            r#"{{"type":"assistant","cwd":"/x/Proj","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{input},"cache_read_input_tokens":100,"output_tokens":10}},"content":[]}}}}"#
        )
    };
    let d = tempfile::tempdir().unwrap();
    let sub = d.path().join("-x-Proj/s1/subagents");
    fs::create_dir_all(&sub).unwrap();
    let main = format!(
        "{}\n{}\n{}\n",
        line("m1", "opus", 50),
        line("m2", "opus", 0),
        r#"{"type":"assistant","message":{"id":"m3","model":"opus","usage":{"input_tokens":1,"output_tokens":1},"content":[{"type":"tool_use","id":"toolu_b","name":"Agent","input":{"subagent_type":"worker-lite"}}]}}"#
    );
    fs::write(d.path().join("-x-Proj/s1.jsonl"), main).unwrap();
    let a = format!("{}\n{}\n", line("a1", "haiku", 20), line("a2", "haiku", 0));
    fs::write(sub.join("agent-a.jsonl"), a).unwrap();
    fs::write(sub.join("agent-a.meta.json"), r#"{"agentType":"worker"}"#).unwrap();
    // No agentType: role comes from the parent's Agent call.
    fs::write(sub.join("agent-b.jsonl"), line("b1", "haiku", 5)).unwrap();
    fs::write(sub.join("agent-b.meta.json"), r#"{"toolUseId":"toolu_b"}"#).unwrap();

    let r = report(&UsageOpts {
        claude_root: Some(d.path().to_owned()),
        codex_root: Some(d.path().join("none")),
        ..Default::default()
    });
    let rows: Vec<_> = r
        .agents
        .iter()
        .map(|a| {
            (
                a.role.as_str(),
                a.spawns,
                a.median_requests,
                a.median_first_context,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("main", 1, 3.0, 150.0),
            ("worker", 1, 2.0, 120.0),
            ("worker-lite", 1, 1.0, 105.0)
        ]
    );
    assert_eq!(r.agents[1].models, ["haiku"]);
    // (20 + 10 + 50) + (0 + 10 + 50)
    assert_eq!(r.agents[1].weighted, 140.0);
    let share: f64 = r.agents.iter().map(|a| a.share).sum();
    assert!((share - 1.0).abs() < 1e-9);
    // Existing views still count subagents in their parent session.
    assert_eq!(r.sessions.len(), 1);
}

const CLAUDE_RESULTS: &str = r#"{"type":"assistant","message":{"id":"r1","model":"opus","usage":{"input_tokens":1,"output_tokens":1},"content":[{"type":"tool_use","id":"a1","name":"Bash","input":{"command":"acus find 'x' --block"}},{"type":"tool_use","id":"a2","name":"Bash","input":{"command":"acus show a.rs:1-5"}},{"type":"tool_use","id":"a3","name":"Read","input":{"file_path":"a"}},{"type":"tool_use","id":"a4","name":"Bash","input":{"command":"cat > notes.md <<'EOF'\nhi\nEOF"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"a1","content":"0123456789"},{"type":"tool_result","tool_use_id":"a2","content":[{"type":"text","text":"héllo"}]},{"type":"tool_result","tool_use_id":"a3","content":"abc"}]}}
"#;

const CODEX_JS: &str = r#"{"type":"session_meta","payload":{"cwd":"/y/Other"}}
{"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"tools.exec_command({cmd: \"acus find 'x' --block\"})","call_id":"c1"}}
{"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c1","output":"héllo"}}
"#;

#[test]
fn result_chars_follow_their_call() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("s.jsonl"), CLAUDE_RESULTS).unwrap();
    let r = report(&UsageOpts {
        claude_root: Some(d.path().into()),
        codex_root: Some(d.path().join("none")),
        ..Default::default()
    });
    assert_eq!(r.categories["acus:find"].result_chars, [10, 0]);
    assert_eq!(r.categories["acus:show"].result_chars, [5, 0]);
    assert_eq!(r.categories["read"].result_chars, [3, 0]);
    assert_eq!(r.result_chars, 18);
    assert_eq!(r.categories["edit-shell"].calls, [1, 0]);
}

#[test]
fn escaped_codex_acus_call_is_counted() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("rollout-a.jsonl"), CODEX_JS).unwrap();
    let r = report(&UsageOpts {
        claude_root: Some(d.path().join("none")),
        codex_root: Some(d.path().into()),
        ..Default::default()
    });
    let s = r.categories["acus:find"];
    assert_eq!((s.calls, s.result_chars), ([0, 1], [0, 5]));
}

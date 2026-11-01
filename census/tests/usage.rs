// SPDX-License-Identifier: MPL-2.0
use census::usage::{Report, UsageOpts, compare, parse_date, report};
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

/// One assistant record of a request sent at `ts` with the given usage JSON.
fn request(ts: &str, id: &str, model: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","model":"{model}","usage":{usage},"content":[]}}}}"#
    )
}

/// Usage JSON of a request: input, cache read, 5-minute and 1-hour cache writes.
fn usage(input: u64, read: u64, w5: u64, w1h: u64) -> String {
    format!(
        r#"{{"input_tokens":{input},"cache_read_input_tokens":{read},"cache_creation":{{"ephemeral_5m_input_tokens":{w5},"ephemeral_1h_input_tokens":{w1h}}},"output_tokens":1}}"#
    )
}

/// An assistant record that starts an Agent call of the subagent type `kind`.
fn spawn(ts: &str, id: &str, kind: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"msg-{id}","model":"claude-opus-5-5","content":[{{"type":"tool_use","id":"{id}","name":"Agent","input":{{"subagent_type":"{kind}"}}}}]}}}}"#
    )
}

/// The user record that answers the Agent call `id`.
fn answer(ts: &str, id: &str) -> String {
    format!(
        r#"{{"type":"user","timestamp":"{ts}","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"done"}}]}}}}"#
    )
}

/// Writes `lines` to `dir/rel`, creating the directories.
#[expect(
    clippy::unwrap_used,
    reason = "a fixture that cannot be written must fail the test"
)]
fn write(dir: &std::path::Path, rel: &str, lines: &[String]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, lines.join("\n") + "\n").unwrap();
}

fn claude_only(dir: &std::path::Path) -> Report {
    report(&UsageOpts {
        claude_root: Some(dir.into()),
        codex_root: Some(dir.join("none")),
        ..Default::default()
    })
}

#[test]
fn prices_per_request_tier_and_cache_ttl() {
    let d = tempfile::tempdir().unwrap();
    let t = "2026-10-14T10:00:00.000Z";
    write(
        d.path(),
        "-x-Proj/p.jsonl",
        &[
            // Opus 5.5 per million: $4 input, $0.20 read, $5 (5m) and $8 (1h) writes, $20 output.
            request(
                t,
                "o",
                "claude-opus-5-5",
                r#"{"input_tokens":1000,"cache_read_input_tokens":2000,"cache_creation":{"ephemeral_5m_input_tokens":1000,"ephemeral_1h_input_tokens":500},"output_tokens":100}"#,
            ),
            // Haiku 5.5 short tier: $0.10 input, $0.50 output.
            request(
                t,
                "h1",
                "claude-haiku-5-5",
                r#"{"input_tokens":1000,"output_tokens":1000}"#,
            ),
            // Haiku 5.5 above 100k context: $0.50 input, $0.05 read, $1.00 for the 1h writes.
            request(
                t,
                "h2",
                "claude-haiku-5-5",
                r#"{"input_tokens":50000,"cache_read_input_tokens":60000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1000},"output_tokens":0}"#,
            ),
            request(t, "g", "gpt-x", r#"{"input_tokens":1,"output_tokens":1}"#),
        ],
    );
    let r = claude_only(d.path());
    // 0.0154 opus + 0.0006 short haiku + 0.029 long haiku.
    assert!((r.cost.usd - 0.045).abs() < 1e-9, "{}", r.cost.usd);
    assert_eq!(r.cost.unpriced, 1);
    let haiku = r
        .model_costs
        .iter()
        .find(|(m, _)| m == "claude-haiku-5-5")
        .unwrap()
        .1;
    assert!((haiku.usd - 0.0296).abs() < 1e-9, "{}", haiku.usd);
}

#[test]
fn handbacks_follow_worker_spawns_within_thirty_minutes() {
    let d = tempfile::tempdir().unwrap();
    let at = |h: u32, m: u32| format!("2026-10-14T{h:02}:{m:02}:00.000Z");
    write(
        d.path(),
        "-x-Proj/s1.jsonl",
        &[
            spawn(&at(10, 0), "w1", "worker"),
            answer(&at(10, 10), "w1"),
            // 25 minutes after the worker finished: a hand-back.
            spawn(&at(10, 35), "h1", "senior-worker"),
            // No worker finished within 30 minutes before it.
            spawn(&at(12, 0), "h2", "senior-worker"),
            // Never answered: the worker counts from its start.
            spawn(&at(13, 0), "w2", "worker"),
            // 29 minutes after that start: a hand-back.
            spawn(&at(13, 29), "h3", "senior-worker"),
            spawn(&at(13, 45), "h4", "senior-worker"),
            spawn(&at(14, 0), "w3", "worker"),
            answer(&at(14, 5), "w3"),
        ],
    );
    // A senior-worker in another session has no worker to follow.
    write(
        d.path(),
        "-x-Proj/s2.jsonl",
        &[spawn(&at(10, 40), "h5", "senior-worker")],
    );
    let r = claude_only(d.path());
    assert_eq!(r.handbacks.len(), 1);
    let w = &r.handbacks[0];
    assert_eq!(
        (w.week.as_str(), w.workers, w.seniors, w.handbacks),
        ("2026-W42", 3, 5, 2)
    );
    assert!((w.rate.unwrap() - 2.0 / 3.0).abs() < 1e-9);
}

#[test]
fn expired_cache_and_busts_per_role() {
    let d = tempfile::tempdir().unwrap();
    let at = |h: u32, m: u32| format!("2026-10-14T{h:02}:{m:02}:00.000Z");
    let opus = "claude-opus-5-5";
    write(
        d.path(),
        "-x-Proj/s1.jsonl",
        &[
            request(&at(10, 0), "m1", opus, &usage(0, 0, 0, 1000)),
            // 30 minutes later, inside the 1 h main-thread TTL: no event.
            request(&at(10, 30), "m2", opus, &usage(10, 9000, 0, 0)),
            // 90 minutes later: the cache was rewritten after it expired.
            request(&at(12, 0), "m3", opus, &usage(10, 9000, 0, 1000)),
            // 10 minutes later, but the writes are most of the context: a bust.
            request(&at(12, 10), "m4", opus, &usage(10, 500, 0, 9000)),
        ],
    );
    let haiku = "claude-haiku-5-5";
    write(
        d.path(),
        "-x-Proj/s1/subagents/agent-a.jsonl",
        &[
            request(&at(10, 0), "a1", haiku, &usage(10, 0, 1000, 0)),
            // 6 minutes later is past the 5 min subagent TTL: an expired rewrite.
            request(&at(10, 6), "a2", haiku, &usage(10, 0, 1000, 0)),
            // 2 minutes later, with the writes most of the context: a bust.
            request(&at(10, 8), "a3", haiku, &usage(10, 0, 900, 0)),
        ],
    );
    write(
        d.path(),
        "-x-Proj/s1/subagents/agent-a.meta.json",
        &[r#"{"agentType":"worker"}"#.to_owned()],
    );
    let r = claude_only(d.path());
    let role = |name: &str| r.agents.iter().find(|a| a.role == name).unwrap().cache;
    let main = role("main");
    assert_eq!((main.expired, main.busts), (1, 1));
    // The 1 h write of m3 at $8 per million.
    assert!(
        (main.expired_usd - 0.008).abs() < 1e-9,
        "{}",
        main.expired_usd
    );
    let worker = role("worker");
    assert_eq!((worker.expired, worker.busts), (1, 1));
    assert_eq!((r.cache.expired, r.cache.busts), (2, 2));
}

#[test]
fn compactions_record_the_context_before_them() {
    let d = tempfile::tempdir().unwrap();
    let at = |h: u32| format!("2026-10-14T{h:02}:00:00.000Z");
    let boundary = |ts: &str, meta: &str| {
        format!(r#"{{"type":"system","subtype":"compact_boundary","timestamp":"{ts}"{meta}}}"#)
    };
    write(
        d.path(),
        "-x-Proj/s3.jsonl",
        &[
            request(&at(10), "c1", "claude-opus-5-5", &usage(5000, 1000, 0, 0)),
            // Without compactMetadata, the context is the last request's: 6000.
            boundary(&at(11), ""),
            boundary(
                &at(12),
                r#","compactMetadata":{"trigger":"auto","preTokens":123456}"#,
            ),
        ],
    );
    let r = claude_only(d.path());
    let s = &r.sessions[0];
    assert_eq!(s.compactions, 2);
    assert_eq!(s.pre_compact, [6000, 123_456]);
    assert_eq!(r.compactions, 2);
}

#[test]
fn a_context_drop_is_a_compaction_not_a_bust() {
    let d = tempfile::tempdir().unwrap();
    let at = |h: u32, m: u32| format!("2026-10-14T{h:02}:{m:02}:00.000Z");
    let opus = "claude-opus-5-5";
    write(
        d.path(),
        "-x-Proj/s4.jsonl",
        &[
            request(&at(10, 0), "k1", opus, &usage(10, 0, 0, 300_000)),
            // 2 minutes later the context is down to 30k: the compaction rewrote most of it.
            request(&at(10, 2), "k2", opus, &usage(10, 500, 0, 30_000)),
        ],
    );
    let r = claude_only(d.path());
    let main = r.agents.iter().find(|a| a.role == "main").unwrap().cache;
    assert_eq!((main.compactions, main.busts, main.expired), (1, 0, 0));
}

#[test]
fn a_request_after_a_compact_boundary_is_a_compaction() {
    let d = tempfile::tempdir().unwrap();
    let at = |h: u32, m: u32| format!("2026-10-14T{h:02}:{m:02}:00.000Z");
    let opus = "claude-opus-5-5";
    write(
        d.path(),
        "-x-Proj/s5.jsonl",
        &[
            request(&at(10, 0), "b1", opus, &usage(10, 9000, 0, 0)),
            format!(
                r#"{{"type":"system","subtype":"compact_boundary","timestamp":"{}"}}"#,
                at(10, 1)
            ),
            // The context is not halved, but a compaction came before this request.
            request(&at(10, 2), "b2", opus, &usage(10, 0, 0, 9500)),
        ],
    );
    let r = claude_only(d.path());
    let main = r.agents.iter().find(|a| a.role == "main").unwrap().cache;
    assert_eq!((main.compactions, main.busts), (1, 0));
}

#[test]
fn synthetic_messages_are_not_requests() {
    let d = tempfile::tempdir().unwrap();
    let at = |h: u32, m: u32| format!("2026-10-14T{h:02}:{m:02}:00.000Z");
    let opus = "claude-opus-5-5";
    write(
        d.path(),
        "-x-Proj/s6.jsonl",
        &[
            request(&at(10, 0), "y1", opus, &usage(10, 0, 0, 9000)),
            // An abort 30 minutes after the first request, and 70 minutes before the second.
            r#"{"type":"assistant","timestamp":"2026-10-14T10:30:00.000Z","message":{"id":"sy","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0},"content":[]}}"#.to_owned(),
            request(&at(11, 10), "y2", opus, &usage(10, 0, 0, 9000)),
        ],
    );
    let r = claude_only(d.path());
    let main = r.agents.iter().find(|a| a.role == "main").unwrap().cache;
    // The gap to the last real request is over the TTL: an expired rewrite, not a bust.
    assert_eq!((main.expired, main.busts), (1, 0));
    assert_eq!(r.total.requests, 2);
}

#[test]
fn a_rewrite_of_the_same_context_within_the_ttl_is_a_bust() {
    let d = tempfile::tempdir().unwrap();
    let at = |h: u32, m: u32| format!("2026-10-14T{h:02}:{m:02}:00.000Z");
    let opus = "claude-opus-5-5";
    write(
        d.path(),
        "-x-Proj/s7.jsonl",
        &[
            request(&at(10, 0), "r1", opus, &usage(10, 9000, 0, 0)),
            // 10 minutes later the context is the same size, and it was written, not read.
            request(&at(10, 10), "r2", opus, &usage(10, 0, 0, 9000)),
        ],
    );
    let r = claude_only(d.path());
    let main = r.agents.iter().find(|a| a.role == "main").unwrap().cache;
    assert_eq!((main.busts, main.compactions, main.expired), (1, 0, 0));
}

#[test]
fn compare_splits_requests_at_the_date() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "-x-Proj/s4.jsonl",
        &[
            request(
                "2026-10-13T23:59:59.999Z",
                "b",
                "claude-opus-5-5",
                &usage(100, 0, 0, 0),
            ),
            request(
                "2026-10-14T00:00:00.000Z",
                "a1",
                "claude-opus-5-5",
                &usage(300, 0, 0, 0),
            ),
            request(
                "2026-10-15T08:00:00.000Z",
                "a2",
                "claude-opus-5-5",
                &usage(500, 0, 0, 0),
            ),
        ],
    );
    let opts = UsageOpts {
        claude_root: Some(d.path().into()),
        codex_root: Some(d.path().join("none")),
        ..Default::default()
    };
    let cut = parse_date("2026-10-14").unwrap();
    let (before, after) = compare(&opts, cut);
    assert_eq!((before.total.requests, before.total.input), (1, 100));
    assert_eq!((after.total.requests, after.total.input), (2, 800));
    assert_eq!((before.sessions.len(), after.sessions.len()), (1, 1));
    assert!(parse_date("2026-13-01").is_none());
    assert!(parse_date("2026-10-14T00:00:00Z").is_none());
}

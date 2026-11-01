// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn roster_lists_subagents_in_the_plan_and_throttles() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    let out = e.tick(t0 + secs(1));
    let plans = of_kind(&out, "plan");
    assert_eq!(plans.len(), 1);
    assert_eq!(
        plans[0]["params"]["update"],
        json!({"sessionUpdate": "plan", "entries": [
            {"content": "Agent: starting (0 tool calls, 0s)", "priority": "medium", "status": "pending"},
            {"content": "Agent: starting (0 tool calls, 0s)", "priority": "medium", "status": "pending"},
        ]})
    );
    e.on_agent(&kid("A", "a1", "Read src/a.rs"), t0 + ms(1_200));
    assert!(
        of_kind(&e.tick(t0 + ms(1_900)), "plan").is_empty(),
        "throttled"
    );
    let plans = of_kind(&e.tick(t0 + ms(2_000)), "plan");
    assert_eq!(plans.len(), 1);
    assert_eq!(
        plans[0]["params"]["update"]["entries"][0],
        json!({"content": "Agent: Read src/a.rs (1 tool call, 0s)",
            "priority": "medium", "status": "in_progress"})
    );
    assert!(
        of_kind(&e.tick(t0 + secs(3)), "plan").is_empty(),
        "unchanged"
    );
}

#[test]
fn roster_can_be_disabled() {
    let t0 = Instant::now();
    let mut e = two_background_with(
        t0,
        Config {
            roster: false,
            ..test_config()
        },
    );
    assert!(of_kind(&e.tick(t0 + secs(2)), "plan").is_empty());
    let plan = update(json!({"sessionUpdate": "plan", "entries": []}));
    assert_eq!(
        e.on_agent(&plan, t0 + secs(3)),
        vec![Action::ToClient {
            line: plan,
            synthesized: false
        }]
    );
}

#[test]
fn adapter_plans_are_merged_and_pass_through_without_subagents() {
    let t0 = Instant::now();
    let todo = json!([{"content": "write tests", "priority": "high", "status": "pending"}]);
    let plan = update(json!({"sessionUpdate": "plan", "entries": todo}));
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    // No subagents: forwarded as the original bytes.
    assert_eq!(
        e.on_agent(&plan, t0),
        vec![Action::ToClient {
            line: plan.clone(),
            synthesized: false
        }]
    );
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    let out = e.on_agent(&plan, t0 + ms(500));
    assert_eq!(out.len(), 1);
    let entries = &client_lines(&out)[0]["params"]["update"]["entries"];
    assert_eq!(entries.as_array().unwrap().len(), 2);
    assert_eq!(entries[0], todo[0], "adapter entries come first");
    assert_eq!(entries[1]["status"], "pending");
    // A later poll only re-sends when something changed.
    assert!(of_kind(&e.tick(t0 + secs(3)), "plan").is_empty());
    e.on_agent(&kid("A", "a1", "Read a"), t0 + secs(3));
    let plans = of_kind(&e.tick(t0 + secs(5)), "plan");
    assert_eq!(plans[0]["params"]["update"]["entries"][0], todo[0]);
    assert_eq!(
        plans[0]["params"]["update"]["entries"][1]["status"],
        "in_progress"
    );
}

#[test]
fn roster_names_the_subagent_from_its_input() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    let input = json!({"subagent_type": "worker", "description": "Fix ledger torn tail",
        "prompt": "Fix the torn tail", "model": "sonnet", "run_in_background": true});
    let text = "Async agent launched successfully.";
    e.on_agent(
        &update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "A",
            "status": "completed", "rawInput": input,
            "rawOutput": [{"type": "text", "text": text}],
            "content": [{"type": "content", "content": {"type": "text", "text": text}}]}),
        ),
        t0,
    );
    let plans = of_kind(&e.tick(t0 + secs(1)), "plan");
    assert_eq!(
        plans[0]["params"]["update"]["entries"][0]["content"],
        "worker (sonnet) - Fix ledger torn tail: starting (0 tool calls, 0s)"
    );
}

#[test]
fn finished_subagents_stay_completed_until_the_following_turn_ends() {
    let t0 = Instant::now();
    let mut e = two_background(t0);
    e.on_agent(&kid_ended("A", "a1", "Read a"), t0 + secs(2));
    e.on_agent(&kid_ended("B", "b1", "Read b"), t0 + secs(5));
    e.on_agent(&task_notification(), t0 + secs(10));
    // A new prompt takes over, so the answer to the first one is a handoff.
    e.on_client(&prompt(2), t0 + secs(10));
    e.on_agent(&response(1), t0 + secs(10));
    let statuses = |out: &[Action]| -> Vec<String> {
        of_kind(out, "plan")
            .last()
            .map(|p| {
                p["params"]["update"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["status"].as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let out = e.tick(t0 + secs(11));
    assert_eq!(statuses(&out), ["completed", "in_progress"]);
    let content = of_kind(&out, "plan")[0]["params"]["update"]["entries"][0]["content"].clone();
    assert_eq!(content, "Agent: done (1 tool call, 9s)");
    // The next turn starts and ends: the finished one is dropped (the
    // other subagent ends with that turn's response).
    e.on_client(&prompt(3), t0 + secs(20));
    e.on_agent(&response(2), t0 + secs(21));
    let out = e.tick(t0 + secs(22));
    assert_eq!(statuses(&out), ["completed", "in_progress"], "handoff");
    e.on_agent(&response(3), t0 + secs(23));
    let out = e.tick(t0 + secs(24));
    assert_eq!(statuses(&out), ["completed"]);
}

#[test]
fn roster_empties_when_every_finished_subagent_is_forgotten() {
    let t0 = Instant::now();
    let mut e = idle_subagent(t0);
    e.tick(t0 + secs(2));
    let out = e.tick(t0 + secs(21));
    assert_eq!(
        of_kind(&out, "plan")[0]["params"]["update"]["entries"][0]["status"],
        "completed"
    );
    let out = e.tick(t0 + secs(82));
    let plans = of_kind(&out, "plan");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0]["params"]["update"]["entries"], json!([]));
    assert!(of_kind(&e.tick(t0 + secs(90)), "plan").is_empty());
}

#[test]
fn notice_is_sent_when_the_client_advertised_support() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&initialize(true), t0);
    let out = notify_a(&mut e, t0);
    let notices = notices_in(&out);
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0],
        json!({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "s",
            "update": {"sessionUpdate": "notice", "severity": "info",
                "title": "Agent finished", "description": "1 tool call, 10s"}}})
    );
    // Not subject to the entry gap: the notice precedes the entry.
    let kinds: Vec<_> = client_lines(&out)
        .iter()
        .map(|m| m["params"]["update"]["sessionUpdate"].clone())
        .collect();
    assert_eq!(kinds[..3], ["tool_call_update", "notice", "tool_call"]);
}

#[test]
fn notice_is_not_sent_without_advertised_support() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&initialize(false), t0);
    assert!(notices_in(&notify_a(&mut e, t0)).is_empty());
}

#[test]
fn notice_mode_overrides_the_advertisement() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        notices: NoticeMode::Always,
        ..test_config()
    });
    assert_eq!(notices_in(&notify_a(&mut e, t0)).len(), 1);
    let mut e = Engine::new(Config {
        notices: NoticeMode::Never,
        ..test_config()
    });
    e.on_client(&initialize(true), t0);
    assert!(notices_in(&notify_a(&mut e, t0)).is_empty());
}

#[test]
fn stop_notice_is_a_warning() {
    let t0 = Instant::now();
    let mut e = Engine::new(Config {
        notices: NoticeMode::Always,
        ..test_config()
    });
    e.on_client(&prompt(1), t0);
    e.on_agent(&real_parent("A"), t0);
    e.on_agent(&real_launch_result("A"), t0);
    e.on_client(&cancel(), t0 + secs(5));
    let out = e.on_client(&cancel(), t0 + secs(6));
    let n = &notices_in(&out)[0]["params"]["update"];
    assert_eq!(n["severity"], "warning");
    assert_eq!(n["title"], "Agent stopped");
}

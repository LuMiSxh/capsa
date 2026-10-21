// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_dead_agent_answers_every_waiting_prompt_with_an_error_and_one_notice() {
    let t0 = Instant::now();
    let mut e = engine();
    e.on_client(&prompt(1), t0);
    e.on_client(
        &line(
            json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
            "params": {"sessionId": "t", "prompt": []}}),
        ),
        t0,
    );
    e.on_client(&prompt(3), t0);
    let out = e.on_agent_exit("code 3");
    let all = client_lines(&out);
    let errors: Vec<_> = all.iter().filter(|m| m.get("error").is_some()).collect();
    assert_eq!(errors.len(), 3, "{out:?}");
    let mut ids: Vec<_> = errors.iter().map(|m| m["id"].clone()).collect();
    ids.sort_by_key(Value::to_string);
    assert_eq!(ids, [json!(1), json!(2), json!(3)]);
    for m in &errors {
        assert_eq!(
            m["error"]["message"],
            "pons: the agent process exited (code 3); restart the thread"
        );
    }
    let notices: Vec<_> = all
        .iter()
        .filter(|m| m["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .collect();
    assert_eq!(notices.len(), 2, "one per session: {out:?}");
    assert!(
        notices[0]["params"]["update"]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("agent process exited (code 3)")
    );
    assert!(out.iter().all(|a| match a {
        Action::ToClient { line, .. } => line.is_ascii(),
        _ => true,
    }));
    assert!(has_note(&out, "unanswered"));
}

#[test]
fn a_dead_agent_with_nothing_waiting_says_nothing() {
    let t0 = Instant::now();
    let mut e = engine();
    assert!(e.on_agent_exit("code 0").is_empty());
    e.on_client(&prompt(1), t0);
    e.on_agent(&response(1), t0);
    assert!(e.on_agent_exit("code 0").is_empty());
    // A prompt pons already answered itself is not answered twice.
    let mut e = finished_text(t0);
    e.tick(t0 + secs(200));
    assert!(e.on_agent_exit("code 0").is_empty());
}

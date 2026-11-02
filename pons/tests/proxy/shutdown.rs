// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

#[cfg(unix)]
use serde_json::json;
#[cfg(unix)]
use tokio::time::sleep;
use tokio::time::timeout;

use super::harness::Harness;
#[cfg(unix)]
use super::harness::{start_with_agent_args, temp_path};

#[tokio::test]
async fn a_dead_agent_answers_the_waiting_prompt_with_an_error_and_pons_exits() {
    let mut h = Harness::start(&[]);
    h.prompt(1, "die").await;
    let seen = h.recv_until(|m| m["id"] == 1).await;
    let notice = seen
        .iter()
        .find(|m| m["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .unwrap_or_else(|| panic!("a notice came first: {seen:?}"));
    let text = notice["params"]["update"]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("agent process exited (code 3)"), "{text}");
    let error = &seen.last().unwrap()["error"];
    assert_eq!(
        error["message"],
        "pons: the agent process exited (code 3); restart the thread"
    );
    assert!(seen.iter().all(|m| m.to_string().is_ascii()));
    let status = timeout(Duration::from_secs(10), h.child.wait())
        .await
        .expect("pons exits")
        .unwrap();
    assert_eq!(status.code(), Some(3));
}

#[tokio::test]
async fn responses_in_the_pipe_when_the_agent_exits_are_delivered_not_replaced_by_an_error() {
    let mut h = Harness::start(&[]);
    h.prompt(1, "reply-exit").await;
    let seen = h.recv_until(|m| m["id"] == 1).await;
    assert_eq!(seen.len(), 193, "every update and the response arrived");
    let last = seen.last().unwrap();
    assert!(
        last["error"].is_null(),
        "the real response, not an error: {last}"
    );
    assert_eq!(last["result"]["stopReason"], "end_turn");
    let status = timeout(Duration::from_secs(10), h.child.wait())
        .await
        .expect("pons exits")
        .unwrap();
    assert_eq!(status.code(), Some(0));
}

#[cfg(unix)]
fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(unix)]
async fn until_dead(pids: &[i64], secs: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if pids.iter().all(|p| !alive(*p)) {
            return true;
        }
        sleep(Duration::from_millis(50)).await;
    }
    false
}

#[cfg(unix)]
async fn pids(h: &mut Harness) -> (i64, i64) {
    h.prompt(1, "spawn-grandchild").await;
    let msg = h.recv_until(|m| m["method"] == "mock/pids").await;
    let p = &msg.last().unwrap()["params"];
    (
        p["agent"].as_i64().unwrap(),
        p["grandchild"].as_i64().unwrap(),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn closing_the_client_stops_the_agent_and_its_grandchild() {
    let log = temp_path("teardown.log");
    let _ = std::fs::remove_file(&log);
    let mut h = start_with_agent_args(
        &["--shutdown-grace-ms", "300", "--log", log.to_str().unwrap()],
        &[],
    );
    let (agent, grandchild) = pids(&mut h).await;
    assert!(alive(agent) && alive(grandchild));
    drop(h.stdin);
    let status = timeout(Duration::from_secs(10), h.child.wait())
        .await
        .expect("pons should exit")
        .unwrap();
    assert!(status.success());
    // The agent exits by itself on EOF, but the grandchild only goes with the
    // process group.
    assert!(
        until_dead(&[agent, grandchild], 10).await,
        "agent {agent} or grandchild {grandchild} survived"
    );
    let text = std::fs::read_to_string(&log).unwrap();
    let _ = std::fs::remove_file(&log);
    assert!(text.contains("exiting: client closed"), "{text}");
    assert!(text.contains("exiting: "), "{text}");
}

#[cfg(unix)]
#[tokio::test]
async fn an_agent_that_ignores_eof_and_sigterm_is_killed() {
    let log = temp_path("stubborn.log");
    let _ = std::fs::remove_file(&log);
    let mut h = start_with_agent_args(
        &["--shutdown-grace-ms", "200", "--log", log.to_str().unwrap()],
        &["--ignore-eof"],
    );
    let (agent, grandchild) = pids(&mut h).await;
    drop(h.stdin);
    let status = timeout(Duration::from_secs(15), h.child.wait())
        .await
        .expect("pons should exit")
        .unwrap();
    assert!(status.success());
    assert!(until_dead(&[agent, grandchild], 10).await);
    let text = std::fs::read_to_string(&log).unwrap();
    let _ = std::fs::remove_file(&log);
    assert!(text.contains("SIGTERM"), "{text}");
    assert!(text.contains("SIGKILL"), "{text}");
}

#[tokio::test]
async fn exits_when_client_closes_stdin() {
    let mut h = Harness::start(&[]);
    drop(h.stdin);
    let status = timeout(Duration::from_secs(5), h.child.wait())
        .await
        .expect("pons should exit")
        .unwrap();
    assert!(status.success());
}

/// Sends `sig` to a running pons: the signal is forwarded to the agent, and the
/// wire log ends with the `exiting:` line that says why pons ended.
#[cfg(unix)]
async fn signal_pons_and_read_the_log(sig: libc::c_int, name: &str) {
    let log = temp_path(&format!("{name}.log"));
    let _ = std::fs::remove_file(&log);
    let mut h = start_with_agent_args(&["--log", log.to_str().unwrap()], &[]);
    h.send(json!({"jsonrpc": "2.0", "id": 1, "method": "echo/anything", "params": {}}))
        .await;
    let _ = h.recv().await;
    let pid = h.child.id().expect("pons pid") as libc::pid_t;
    // SAFETY: kill(2) signals the pons child this test spawned.
    assert_eq!(unsafe { libc::kill(pid, sig) }, 0);
    timeout(Duration::from_secs(15), h.child.wait())
        .await
        .expect("pons should exit")
        .unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    let _ = std::fs::remove_file(&log);
    assert!(
        text.contains(&format!("forwarding {name} to the agent")),
        "{text}"
    );
    let last = text.lines().last().unwrap_or_default();
    assert!(last.contains("exiting: "), "{text}");
}

#[cfg(unix)]
#[tokio::test]
async fn sigterm_is_forwarded_and_the_log_says_why_pons_exited() {
    signal_pons_and_read_the_log(libc::SIGTERM, "SIGTERM").await;
}

#[cfg(unix)]
#[tokio::test]
async fn sighup_is_forwarded_and_the_log_says_why_pons_exited() {
    signal_pons_and_read_the_log(libc::SIGHUP, "SIGHUP").await;
}

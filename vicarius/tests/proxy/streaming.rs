// SPDX-License-Identifier: MPL-2.0
//! Streaming without buffering, and usage read across chunk boundaries.

use crate::mock::{Mock, Reply, Step};
use crate::support::{DEEPSEEK_BODY, KEY, MESSAGES_BODY, start};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

const START: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":3,\"cache_read_input_tokens\":5,\"output_tokens\":1}}}\n\n";
const DELTA: &str =
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":42}}\n\n";

async fn idle() -> Mock {
    Mock::start(|_| Reply::full(200, &[], b"{}")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_event_arrives_before_the_upstream_finishes() {
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    let anthropic = Mock::start(move |_| {
        Reply::steps(
            200,
            &[("content-type", b"text/event-stream")],
            vec![
                Step::Data(START.as_bytes().to_vec()),
                Step::Wait(gate.clone()),
                Step::Data(DELTA.as_bytes().to_vec()),
            ],
        )
    })
    .await;
    let proxy = start(&anthropic, &idle().await, Some(KEY), true).await;
    let mut resp = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(MESSAGES_BODY)
        .send()
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), resp.chunk())
        .await
        .expect("the first event was held back")
        .unwrap()
        .unwrap();
    assert_eq!(first.as_ref(), START.as_bytes());
    release.notify_one();
    let second = tokio::time::timeout(Duration::from_secs(5), resp.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(second.as_ref(), DELTA.as_bytes());
    assert!(resp.chunk().await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn usage_is_read_when_events_split_across_chunks() {
    let text = format!("{START}{DELTA}");
    let cuts = [7, START.len() + 20, text.len() - 3];
    let deepseek = Mock::start(move |_| {
        Reply::steps(
            200,
            &[("content-type", b"text/event-stream")],
            split(&text, &cuts),
        )
    })
    .await;
    let proxy = start(&idle().await, &deepseek, Some(KEY), true).await;
    let resp = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert_eq!(body, format!("{START}{DELTA}"));
    let line = proxy.log_lines(1).await.remove(0);
    assert_eq!(line["event"], "proxy.request");
    assert_eq!(line["route"], "deepseek");
    assert_eq!(line["model"], "deepseek-flash");
    assert_eq!(line["status"], 200);
    assert_eq!(line["input_tokens"], 12);
    assert_eq!(line["cache_creation_tokens"], 3);
    assert_eq!(line["cache_read_tokens"], 5);
    assert_eq!(line["output_tokens"], 42);
}

fn split(text: &str, cuts: &[usize]) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut from = 0;
    for &at in cuts.iter().chain([&text.len()]) {
        steps.push(Step::Data(text.as_bytes()[from..at].to_vec()));
        steps.push(Step::Pause(20));
        from = at;
    }
    steps
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_line_carries_the_claude_code_identifiers() {
    let anthropic =
        Mock::start(|_| Reply::full(200, &[("content-type", b"application/json")], b"{}")).await;
    let proxy = start(&anthropic, &idle().await, None, false).await;
    proxy
        .client
        .post(proxy.url("/v1/messages"))
        .header("x-claude-code-session-id", "sess-42")
        .header("x-claude-code-agent-id", "agent-7")
        .header("anthropic-version", "2023-06-01")
        .body(MESSAGES_BODY)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let line = proxy.log_lines(1).await.remove(0);
    assert_eq!(line["session_id"], "sess-42");
    assert_eq!(line["agent_id"], "agent-7");
    assert_eq!(line["header_values"]["anthropic-version"], "2023-06-01");
    assert_eq!(line["route"], "anthropic");
    assert!(line["duration_ms"].is_u64());
}

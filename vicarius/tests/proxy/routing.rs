// SPDX-License-Identifier: MPL-2.0
//! Routing, passthrough, the closed gate and the status endpoint.

use crate::mock::{Mock, Reply};
use crate::support::{DEEPSEEK_BODY, KEY, MESSAGES_BODY, start};
use vicarius::gate::Closed;

const CLOSED_BODY: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"DeepSeek is currently unavailable (balance not checked yet). Use worker or senior-worker."}}"#;

async fn idle() -> Mock {
    Mock::start(|_| Reply::full(200, &[], b"{}")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_passthrough_keeps_request_and_response_bytes() {
    let gz: &[u8] = &[0x1f, 0x8b, 0x08, 0x00, 0xff, 0xfe, 0x00];
    let anthropic = Mock::start(move |_| {
        Reply::full(
            200,
            &[
                ("content-encoding", b"gzip"),
                ("content-type", b"application/json"),
                ("retry-after", b"7"),
                ("x-should-retry", b"true"),
                ("anthropic-ratelimit-requests-remaining", b"9"),
                ("x-upstream-extra", b"yes"),
            ],
            gz,
        )
    })
    .await;
    let deepseek = idle().await;
    let proxy = start(&anthropic, &deepseek, Some(KEY), true).await;

    let resp = proxy
        .client
        .post(proxy.url("/v1/messages?beta=true"))
        .header("authorization", "Bearer oauth-token")
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("x-unknown-header", "kept")
        .body(MESSAGES_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let h = resp.headers().clone();
    assert_eq!(h["content-encoding"], "gzip");
    assert_eq!(h["retry-after"], "7");
    assert_eq!(h["x-should-retry"], "true");
    assert_eq!(h["anthropic-ratelimit-requests-remaining"], "9");
    assert_eq!(h["x-upstream-extra"], "yes");
    assert_eq!(resp.bytes().await.unwrap().as_ref(), gz);

    let seen = anthropic.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].target, "/v1/messages?beta=true");
    assert_eq!(seen[0].body, MESSAGES_BODY);
    assert_eq!(seen[0].headers["authorization"], "Bearer oauth-token");
    assert_eq!(seen[0].headers["anthropic-beta"], "oauth-2025-04-20");
    assert_eq!(seen[0].headers["x-unknown-header"], "kept");
    assert!(deepseek.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_errors_pass_through_unchanged() {
    let anthropic = Mock::start(|_| {
        Reply::full(
            429,
            &[("retry-after", b"30"), ("x-should-retry", b"false")],
            br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
        )
    })
    .await;
    let proxy = start(&anthropic, &idle().await, None, false).await;
    let resp = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(MESSAGES_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 429);
    assert_eq!(resp.headers()["retry-after"], "30");
    assert_eq!(resp.headers()["x-should-retry"], "false");
    assert!(resp.text().await.unwrap().contains("slow down"));
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_upstream_gives_an_anthropic_shaped_502() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = vicarius::proxy::ProxyConfig::new(0, None);
    config.anthropic_base = "http://127.0.0.1:1".to_owned();
    config.log_file = dir.path().join("proxy.jsonl");
    let gate = std::sync::Arc::new(vicarius::gate::Gate::new(false));
    let proxy = vicarius::proxy::Proxy::bind(config, gate).await.unwrap();
    let addr = proxy.local_addr().unwrap();
    tokio::spawn(proxy.run(std::future::pending()));
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .body(MESSAGES_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
    let body: serde_json::Value = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "api_error");
}

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_route_swaps_credentials_and_keeps_other_headers() {
    let deepseek =
        Mock::start(|_| Reply::full(200, &[("content-type", b"application/json")], b"{}")).await;
    let anthropic = idle().await;
    let proxy = start(&anthropic, &deepseek, Some(KEY), true).await;
    let resp = proxy
        .client
        .post(proxy.url("/v1/messages?x=1"))
        .header("authorization", "Bearer oauth-token")
        .header("x-api-key", "client-key")
        .header("cookie", "s=1")
        .header("anthropic-beta", "oauth-2025-04-20")
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let seen = deepseek.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].target, "/anthropic/v1/messages?x=1");
    assert_eq!(seen[0].headers["x-api-key"], KEY);
    assert!(!seen[0].headers.contains_key("authorization"));
    assert!(!seen[0].headers.contains_key("cookie"));
    assert_eq!(seen[0].headers["anthropic-beta"], "oauth-2025-04-20");
    assert_eq!(seen[0].body, DEEPSEEK_BODY);
    assert!(anthropic.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_404_for_count_tokens_passes_through() {
    let deepseek = Mock::start(|_| Reply::full(404, &[], b"nope")).await;
    let proxy = start(&idle().await, &deepseek, Some(KEY), true).await;
    let resp = proxy
        .client
        .post(proxy.url("/v1/messages/count_tokens"))
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.text().await.unwrap(), "nope");
    assert!(proxy.gate.closed().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_gate_refuses_without_calling_deepseek() {
    let deepseek = idle().await;
    let proxy = start(&idle().await, &deepseek, Some(KEY), false).await;
    let resp = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(resp.headers()["content-type"], "application/json");
    assert_eq!(resp.headers()["x-should-retry"], "false");
    assert_eq!(resp.text().await.unwrap(), CLOSED_BODY);
    assert!(deepseek.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_gate_names_the_reason() {
    let proxy = start(&idle().await, &idle().await, None, false).await;
    let ask = || async {
        let resp = proxy
            .client
            .post(proxy.url("/v1/messages"))
            .body(DEEPSEEK_BODY)
            .send()
            .await
            .unwrap();
        resp.text().await.unwrap()
    };
    assert!(ask().await.contains("(no API key configured)"));
    proxy.gate.close(Closed::LowBalance { minimum: 1.0 }, 5);
    assert!(ask().await.contains("(balance below $1)"));
}

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_402_closes_the_gate_and_the_next_request_is_refused_locally() {
    let deepseek = Mock::start(|_| Reply::full(402, &[], b"insufficient balance")).await;
    let proxy = start(&idle().await, &deepseek, Some(KEY), true).await;
    for _ in 0..2 {
        let resp = proxy
            .client
            .post(proxy.url("/v1/messages"))
            .body(DEEPSEEK_BODY)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        assert_eq!(resp.headers()["x-should-retry"], "false");
        let text = resp.text().await.unwrap();
        assert!(text.contains("(payment required)"), "{text}");
        assert!(!text.contains("insufficient balance"));
    }
    assert_eq!(deepseek.seen().len(), 1);
    assert_eq!(proxy.gate.closed(), Some(Closed::PaymentRequired));
}

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_401_closes_the_gate_as_key_rejected() {
    let deepseek = Mock::start(|_| Reply::full(401, &[], b"bad key")).await;
    let proxy = start(&idle().await, &deepseek, Some(KEY), true).await;
    let resp = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    assert!(resp.text().await.unwrap().contains("(key rejected)"));
    assert_eq!(proxy.gate.closed(), Some(Closed::KeyRejected));
}

#[tokio::test(flavor = "multi_thread")]
async fn other_paths_go_to_anthropic_without_parsing_the_body() {
    let anthropic = idle().await;
    let deepseek = idle().await;
    let proxy = start(&anthropic, &deepseek, Some(KEY), true).await;
    let c = &proxy.client;
    c.post(proxy.url("/v1/messages/batches"))
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    c.post(proxy.url("/v1/other"))
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    c.get(proxy.url("/v1/models")).send().await.unwrap();
    c.put(proxy.url("/v1/messages"))
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    let broken: &[u8] = b"{not json";
    c.post(proxy.url("/v1/messages"))
        .body(broken)
        .send()
        .await
        .unwrap();
    let seen = anthropic.seen();
    assert_eq!(seen.len(), 5);
    assert_eq!(seen[2].method, "GET");
    assert_eq!(seen[3].method, "PUT");
    assert_eq!(seen[4].body, broken);
    assert!(deepseek.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reflects_the_gate_and_the_counters() {
    let anthropic = Mock::start(|_| {
        Reply::full(
            200,
            &[("content-type", b"application/json")],
            br#"{"usage":{"input_tokens":11,"output_tokens":22}}"#,
        )
    })
    .await;
    let proxy = start(&anthropic, &idle().await, Some(KEY), false).await;
    let first = proxy.status().await;
    assert_eq!(first["gate"]["open"], false);
    assert_eq!(first["gate"]["reason"], "not_checked");
    assert_eq!(first["counters"]["anthropic"]["requests"], 0);

    proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(MESSAGES_BODY)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    proxy.gate.open(3.25, "USD", 99);
    proxy.log_lines(2).await;

    let status = proxy.status().await;
    assert_eq!(status["gate"]["open"], true);
    assert_eq!(status["gate"]["balance"], 3.25);
    assert_eq!(status["gate"]["currency"], "USD");
    assert_eq!(status["gate"]["checked_at"], 99);
    assert_eq!(status["counters"]["anthropic"]["requests"], 1);
    assert_eq!(status["counters"]["anthropic"]["input_tokens"], 11);
    assert_eq!(status["counters"]["anthropic"]["output_tokens"], 22);
    assert_eq!(status["counters"]["deepseek"]["requests"], 1);
    assert_eq!(status["counters"]["deepseek"]["refused"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn binds_only_to_loopback() {
    let proxy = start(&idle().await, &idle().await, None, false).await;
    assert_eq!(proxy.addr.ip(), std::net::Ipv4Addr::LOCALHOST);
    // Connecting a UDP socket sends nothing; it only reveals the outbound interface address.
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    if probe.connect("192.0.2.1:9").is_ok() {
        let ip = probe.local_addr().unwrap().ip();
        if !ip.is_loopback() && !ip.is_unspecified() {
            let other = std::net::SocketAddr::new(ip, proxy.addr.port());
            assert!(tokio::net::TcpStream::connect(other).await.is_err());
        }
    }
}

// SPDX-License-Identifier: MPL-2.0
//! Routing, passthrough, the closed gate and the status endpoint.

use crate::mock::{Mock, Reply};
use crate::support::{
    DEEPSEEK_BODY, KEY, MESSAGES_BODY, deepseek_provider, start, start_providers,
};
use std::sync::Arc;
use vicarius::config::{Auth, Compat};
use vicarius::gate::{Closed, Gate};
use vicarius::proxy::{ProviderId, ProviderKey, ProviderSpec};

const CLOSED_BODY: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"provider `deepseek` is unavailable (balance not checked yet); its key comes from DEEPSEEK_API_KEY"}}"#;

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
    let mut config = vicarius::proxy::ProxyConfig::new(0, Vec::new());
    config.anthropic_base = "http://127.0.0.1:1".to_owned();
    config.log_file = dir.path().join("proxy.jsonl");
    let proxy = vicarius::proxy::Proxy::bind(config).await.unwrap();
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

fn inline_body(model: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": [{
            "type": "tool_definition",
            "name": "Lookup",
            "description": "Finds a record",
            "input_schema": {"type": "object"}
        }]}]
    }))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_route_moves_inline_tool_definitions_into_tools() {
    let deepseek = idle().await;
    let anthropic = idle().await;
    let proxy = start(&anthropic, &deepseek, Some(KEY), true).await;
    let resp = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(inline_body("deepseek-flash"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let seen = deepseek.seen();
    assert_eq!(seen.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(body["tools"][0]["name"], "Lookup");
    assert_eq!(body["messages"][0]["content"][0]["text"], "(tool loaded)");
    assert!(anthropic.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_route_forwards_inline_tool_definitions_unchanged() {
    let anthropic = idle().await;
    let deepseek = idle().await;
    let proxy = start(&anthropic, &deepseek, Some(KEY), true).await;
    let body = inline_body("claude-opus-4");
    let resp = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let seen = anthropic.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].body, body);
    assert!(deepseek.seen().is_empty());
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
    assert!(
        ask()
            .await
            .contains("provider `deepseek` is unavailable (no API key configured)")
    );
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
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[2].method, "PUT");
    assert_eq!(seen[3].body, broken);
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
    assert_eq!(first["providers"]["deepseek"]["gated"], true);
    assert_eq!(
        first["providers"]["deepseek"]["gate"]["reason"],
        "not_checked"
    );
    assert_eq!(first["counters"]["routes"]["anthropic"]["requests"], 0);

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
    assert_eq!(status["providers"]["deepseek"]["gate"]["balance"], 3.25);
    assert_eq!(status["providers"]["deepseek"]["gate"]["currency"], "USD");
    assert_eq!(status["counters"]["routes"]["anthropic"]["requests"], 1);
    assert_eq!(
        status["counters"]["routes"]["anthropic"]["input_tokens"],
        11
    );
    assert_eq!(
        status["counters"]["routes"]["anthropic"]["output_tokens"],
        22
    );
    assert_eq!(status["counters"]["routes"]["deepseek"]["requests"], 1);
    assert_eq!(status["counters"]["routes"]["deepseek"]["refused"], 1);
}

/// Builds a provider with `KEY` and no models, so tests control every field they exercise.
fn spec(
    id: &str,
    prefixes: &[&str],
    mock: &Mock,
    gate: Option<Arc<Gate>>,
    auth: Auth,
    compat: Compat,
) -> ProviderSpec {
    ProviderSpec {
        id: ProviderId::new(id),
        prefixes: prefixes.iter().map(|prefix| (*prefix).to_owned()).collect(),
        base_url: format!("{}/anthropic", mock.url()),
        key_env: format!("{}_API_KEY", id.to_ascii_uppercase()),
        auth,
        models: Vec::new(),
        gate,
        compat,
        key: ProviderKey::new(KEY),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn longest_prefix_wins_and_an_unknown_model_logs_and_goes_to_anthropic() {
    let (anthropic, broad, narrow) = (idle().await, idle().await, idle().await);
    let gate = Arc::new(Gate::new(true));
    gate.open(5.0, "USD", 1);
    let providers = vec![
        spec(
            "broad",
            &["deepseek-"],
            &broad,
            Some(gate.clone()),
            Auth::XApiKey,
            Compat::default(),
        ),
        spec(
            "narrow",
            &["deepseek-v4"],
            &narrow,
            Some(gate.clone()),
            Auth::XApiKey,
            Compat::default(),
        ),
    ];
    let proxy = start_providers(&anthropic, providers, gate).await;

    proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(r#"{"model":"deepseek-v4-pro"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(narrow.seen().len(), 1);
    assert!(broad.seen().is_empty());

    proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(r#"{"model":"deepsek-flash"}"#)
        .send()
        .await
        .unwrap();
    let unknown = proxy.event_lines("unknown_model", 1).await;
    assert_eq!(unknown[0]["level"], "warn");
    assert_eq!(unknown[0]["model"], "deepsek-flash");
    assert_eq!(proxy.log_lines(2).await[1]["route"], "anthropic");
    assert_eq!(anthropic.seen().len(), 1);

    proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(MESSAGES_BODY)
        .send()
        .await
        .unwrap();
    proxy.log_lines(3).await;
    let unknown = proxy
        .read_log()
        .into_iter()
        .filter(|line| line["event"] == "unknown_model")
        .count();
    assert_eq!(unknown, 1, "a Claude model must not warn");
    assert_eq!(anthropic.seen().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn bearer_auth_and_compat_flags_apply_per_provider() {
    let (anthropic, moonshot) = (idle().await, idle().await);
    let gate = Arc::new(Gate::new(true));
    gate.open(5.0, "USD", 1);
    let compat = Compat {
        inline_tools: false,
        strip_cache_control: true,
        strip_thinking: false,
    };
    let mut provider = spec(
        "moonshot",
        &["kimi-"],
        &moonshot,
        Some(gate.clone()),
        Auth::Bearer,
        compat,
    );
    provider.models = vec!["kimi-k2".to_owned()];
    let proxy = start_providers(&anthropic, vec![provider], gate).await;

    let body = br#"{"model":"kimi-k2","thinking":{"type":"enabled","budget_tokens":1024},"messages":[{"role":"user","content":[{"type":"tool_definition","name":"Lookup","input_schema":{}},{"type":"text","text":"hi","cache_control":{"type":"ephemeral"}},{"type":"thinking","thinking":"keep"}]}]}"#;
    let resp = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .header("authorization", "Bearer client-oauth")
        .header("x-api-key", "client-key")
        .header("cookie", "s=1")
        .body(body.as_slice())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let seen = moonshot.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].headers["authorization"], format!("Bearer {KEY}"));
    assert!(!seen[0].headers.contains_key("x-api-key"));
    assert!(!seen[0].headers.contains_key("cookie"));
    let sent: serde_json::Value = serde_json::from_slice(&seen[0].body).unwrap();
    let content = &sent["messages"][0]["content"];
    assert_eq!(content[0]["type"], "tool_definition");
    assert!(content[1].get("cache_control").is_none());
    assert_eq!(content[2]["type"], "thinking");
    assert_eq!(sent["thinking"]["budget_tokens"], 1024);
    assert!(sent.get("tools").is_none());
    assert_eq!(proxy.log_lines(1).await[0]["route"], "moonshot");
    assert!(anthropic.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_key_refuses_with_the_provider_and_its_variable() {
    let (anthropic, moonshot) = (idle().await, idle().await);
    let mut provider = spec(
        "moonshot",
        &["kimi-"],
        &moonshot,
        None,
        Auth::XApiKey,
        Compat::default(),
    );
    provider.key = None;
    let proxy = start_providers(&anthropic, vec![provider], Arc::new(Gate::new(true))).await;

    let resp = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(r#"{"model":"kimi-k2"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(resp.headers()["x-should-retry"], "false");
    let body: serde_json::Value = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(
        body["error"]["message"],
        "provider `moonshot` has no key; set MOONSHOT_API_KEY"
    );
    assert!(moonshot.seen().is_empty());
    assert!(anthropic.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_gate_refuses_only_its_provider() {
    let (anthropic, deepseek, moonshot) = (idle().await, idle().await, idle().await);
    let closed = Arc::new(Gate::new(true));
    let open = Arc::new(Gate::new(true));
    open.open(5.0, "USD", 1);
    let providers = vec![
        deepseek_provider(&deepseek, Some(KEY), closed),
        spec(
            "moonshot",
            &["kimi-"],
            &moonshot,
            Some(open.clone()),
            Auth::XApiKey,
            Compat::default(),
        ),
    ];
    let proxy = start_providers(&anthropic, providers, open).await;

    let refused = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    assert!(deepseek.seen().is_empty());
    let allowed = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .body(r#"{"model":"kimi-k2"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);
    assert_eq!(moonshot.seen().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn models_lists_every_configured_model_in_the_openai_shape() {
    let (anthropic, deepseek) = (idle().await, idle().await);
    let proxy = start(&anthropic, &deepseek, Some(KEY), true).await;
    let resp = proxy
        .client
        .get(proxy.url("/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "application/json");
    let body: serde_json::Value = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(body["object"], "list");
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    let created = data[0]["created"].as_u64().unwrap();
    assert_eq!(
        data[0],
        serde_json::json!({
            "id": "deepseek-flash",
            "object": "model",
            "created": created,
            "owned_by": "deepseek"
        })
    );
    assert_eq!(data[1]["id"], "deepseek-v4-pro");
    assert_eq!(data[1]["object"], "model");
    assert_eq!(data[1]["created"], created);
    assert_eq!(data[1]["owned_by"], "deepseek");
    assert!(deepseek.seen().is_empty() && anthropic.seen().is_empty());
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

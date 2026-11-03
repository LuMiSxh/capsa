// SPDX-License-Identifier: MPL-2.0

use crate::mock::{Mock, Reply};
use crate::support::{DEEPSEEK_BODY, KEY, MESSAGES_BODY, start};

async fn ok_mock() -> Mock {
    Mock::start(|_| Reply::full(200, &[("content-type", b"application/json")], b"{}")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_host_gets_403_and_nothing_goes_upstream() {
    let (anthropic, deepseek) = (ok_mock().await, ok_mock().await);
    let proxy = start(&anthropic, &deepseek, Some(KEY), true).await;
    let port = proxy.addr.port();
    for host in [
        format!("evil.example:{port}"),
        "127.0.0.1".to_owned(),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        format!("localhost.evil.example:{port}"),
    ] {
        let resp = proxy
            .client
            .post(proxy.url("/v1/messages"))
            .header("host", &host)
            .header("content-type", "text/plain")
            .body(DEEPSEEK_BODY)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403, "{host}");
        let body: serde_json::Value = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "permission_error");
    }
    assert!(deepseek.seen().is_empty() && anthropic.seen().is_empty());
    let status = proxy.status().await;
    assert_eq!(status["counters"]["forbidden"], 4);
    assert_eq!(status["counters"]["deepseek"]["requests"], 0);
    let lines = proxy.log_lines(4).await;
    assert_eq!(lines[0]["status"], 403);
    assert_eq!(lines[0]["route"], "none");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_with_an_origin_gets_403_on_messages_and_refresh() {
    let (anthropic, deepseek) = (ok_mock().await, ok_mock().await);
    let proxy = start(&anthropic, &deepseek, Some(KEY), true).await;
    let messages = proxy
        .client
        .post(proxy.url("/v1/messages"))
        .header("origin", "https://evil.example")
        .header("content-type", "text/plain")
        .body(DEEPSEEK_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(messages.status(), 403);
    let refresh = proxy
        .client
        .post(proxy.url("/vicarius/refresh"))
        .header("origin", "null")
        .send()
        .await
        .unwrap();
    assert_eq!(refresh.status(), 403);
    let status = proxy
        .client
        .get(proxy.url("/vicarius/status"))
        .header("origin", "http://127.0.0.1")
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), 403);
    assert!(deepseek.seen().is_empty() && anthropic.seen().is_empty());
    assert_eq!(proxy.status().await["counters"]["forbidden"], 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_loopback_host_without_an_origin_routes_as_before() {
    let (anthropic, deepseek) = (ok_mock().await, ok_mock().await);
    let proxy = start(&anthropic, &deepseek, Some(KEY), true).await;
    for (host, body) in [
        (format!("127.0.0.1:{}", proxy.addr.port()), MESSAGES_BODY),
        (format!("localhost:{}", proxy.addr.port()), DEEPSEEK_BODY),
    ] {
        let resp = proxy
            .client
            .post(proxy.url("/v1/messages"))
            .header("host", host)
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
    assert_eq!((anthropic.seen().len(), deepseek.seen().len()), (1, 1));
    assert_eq!(proxy.status().await["counters"]["forbidden"], 0);
}

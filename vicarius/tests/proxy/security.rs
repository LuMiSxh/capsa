// SPDX-License-Identifier: MPL-2.0
//! Secrets never reach the log, the status endpoint or `Debug` output.

use crate::mock::{Mock, Reply};
use crate::support::{DEEPSEEK_BODY, MESSAGES_BODY, start};
use vicarius::proxy::{DeepseekKey, ProxyConfig};

const OAUTH: &str = "Bearer sk-ant-oat-SECRETOAUTH123";
const API_KEY: &str = "SECRETAPIKEY456";
const COOKIE: &str = "session=SECRETCOOKIE789";
const DS_KEY: &str = "sk-DISTINCTDEEPSEEKKEY000";

#[tokio::test(flavor = "multi_thread")]
async fn no_secret_value_reaches_the_log() {
    let ok = || Mock::start(|_| Reply::full(200, &[("content-type", b"application/json")], b"{}"));
    let anthropic = ok().await;
    let deepseek = ok().await;
    let proxy = start(&anthropic, &deepseek, Some(DS_KEY), true).await;
    let send = |body: &'static [u8]| {
        proxy
            .client
            .post(proxy.url("/v1/messages"))
            .header("authorization", OAUTH)
            .header("x-api-key", API_KEY)
            .header("cookie", COOKIE)
            .header("anthropic-beta", "oauth-2025-04-20")
            .body(body)
            .send()
    };
    send(MESSAGES_BODY).await.unwrap().bytes().await.unwrap();
    send(DEEPSEEK_BODY).await.unwrap().bytes().await.unwrap();
    proxy.gate.close(vicarius::gate::Closed::NotChecked, 1);
    send(DEEPSEEK_BODY).await.unwrap().bytes().await.unwrap();
    let lines = proxy.log_lines(3).await;

    let log = std::fs::read_to_string(&proxy.log).unwrap();
    let status = proxy.status().await.to_string();
    for secret in [
        OAUTH,
        API_KEY,
        COOKIE,
        DS_KEY,
        "SECRETOAUTH123",
        "SECRETCOOKIE789",
    ] {
        assert!(!log.contains(secret), "{secret} reached the log");
        assert!(!status.contains(secret), "{secret} reached the status");
    }
    let names = lines[0]["headers"].as_array().unwrap();
    for name in ["authorization", "x-api-key", "cookie"] {
        assert!(
            names.iter().any(|n| n == name),
            "{name} missing from header names"
        );
    }
    assert_eq!(
        lines[0]["header_values"]["anthropic-beta"],
        "oauth-2025-04-20"
    );
}

#[test]
fn the_key_never_shows_in_debug_output() {
    let key = DeepseekKey::new(DS_KEY);
    let config = ProxyConfig::new(0, key);
    assert!(!format!("{config:?}").contains(DS_KEY));
}

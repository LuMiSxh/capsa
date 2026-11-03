// SPDX-License-Identifier: MPL-2.0
//! Starts the proxy against mock upstreams and reads what it logged.

use crate::mock::Mock;
use serde_json::Value;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use vicarius::gate::Gate;
use vicarius::proxy::{DeepseekKey, Proxy, ProxyConfig};

pub const KEY: &str = "sk-DEEPSEEKSECRET000";

pub struct Harness {
    pub addr: SocketAddr,
    pub gate: Arc<Gate>,
    pub log: PathBuf,
    pub client: reqwest::Client,
    _dir: tempfile::TempDir,
    _stop: oneshot::Sender<()>,
}

/// Starts a proxy whose gate is open when `open` is set. `key` is the DeepSeek key.
pub async fn start(anthropic: &Mock, deepseek: &Mock, key: Option<&str>, open: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("proxy.jsonl");
    let key = key.and_then(DeepseekKey::new);
    let gate = Arc::new(Gate::new(key.is_some()));
    if open {
        gate.open(5.0, "USD", 1);
    }
    let mut config = ProxyConfig::new(0, key);
    config.anthropic_base = anthropic.url();
    config.deepseek_base = format!("{}/anthropic", deepseek.url());
    config.log_file = log.clone();
    let proxy = Proxy::bind(config, gate.clone()).await.unwrap();
    let addr = proxy.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(proxy.run(async {
        let _ = stopped.await;
    }));
    Harness {
        addr,
        gate,
        log,
        client: reqwest::Client::new(),
        _dir: dir,
        _stop: stop,
    }
}

impl Harness {
    pub fn url(&self, target: &str) -> String {
        format!("http://{}{target}", self.addr)
    }

    /// Waits until the log holds `count` lines and returns them.
    pub async fn log_lines(&self, count: usize) -> Vec<Value> {
        for _ in 0..200 {
            let text = std::fs::read_to_string(&self.log).unwrap_or_default();
            let lines: Vec<Value> = text
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            if lines.len() >= count {
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the log never reached {count} lines");
    }

    pub async fn status(&self) -> Value {
        let body = self
            .client
            .get(self.url("/vicarius/status"))
            .send()
            .await
            .unwrap();
        serde_json::from_slice(&body.bytes().await.unwrap()).unwrap()
    }
}

pub const MESSAGES_BODY: &[u8] = br#"{ "model":"claude-opus-4", "messages":[] }"#;
pub const DEEPSEEK_BODY: &[u8] = br#"{"model":"deepseek-flash","messages":[]}"#;

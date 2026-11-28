// SPDX-License-Identifier: MPL-2.0
//! Starts the proxy against mock upstreams and reads what it logged.

use crate::mock::Mock;
use serde_json::Value;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use vicarius::config::{Auth, Compat};
use vicarius::gate::Gate;
use vicarius::proxy::{ProviderId, ProviderKey, ProviderSpec, Proxy, ProxyConfig};

pub const KEY: &str = "sk-DEEPSEEKSECRET000";
pub const KEY_ENV: &str = "DEEPSEEK_API_KEY";

pub struct Harness {
    pub addr: SocketAddr,
    pub gate: Arc<Gate>,
    pub log: PathBuf,
    pub client: reqwest::Client,
    _dir: tempfile::TempDir,
    _stop: oneshot::Sender<()>,
}

/// Returns the DeepSeek provider that points at `deepseek` and uses its compat profile.
pub fn deepseek_provider(deepseek: &Mock, key: Option<&str>, gate: Arc<Gate>) -> ProviderSpec {
    ProviderSpec {
        id: ProviderId::new("deepseek"),
        prefixes: vec!["deepseek-".to_owned()],
        base_url: format!("{}/anthropic", deepseek.url()),
        key_env: KEY_ENV.to_owned(),
        auth: Auth::XApiKey,
        models: vec!["deepseek-flash".to_owned(), "deepseek-v4-pro".to_owned()],
        gate: Some(gate),
        compat: Compat {
            inline_tools: true,
            strip_cache_control: true,
            strip_thinking: true,
        },
        key: key.and_then(ProviderKey::new),
    }
}

/// Starts a proxy whose gate is open when `open` is set. `key` is the DeepSeek key.
pub async fn start(anthropic: &Mock, deepseek: &Mock, key: Option<&str>, open: bool) -> Harness {
    let gate = Arc::new(Gate::new(key.is_some()));
    if open {
        gate.open(5.0, "USD", 1);
    }
    let provider = deepseek_provider(deepseek, key, gate.clone());
    start_providers(anthropic, vec![provider], gate).await
}

/// Starts a proxy with arbitrary providers. `gate` is the one the harness exposes.
pub async fn start_providers(
    anthropic: &Mock,
    providers: Vec<ProviderSpec>,
    gate: Arc<Gate>,
) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("proxy.jsonl");
    let mut config = ProxyConfig::new(0, providers);
    config.anthropic_base = anthropic.url();
    config.log_file = log.clone();
    let proxy = Proxy::bind(config).await.unwrap();
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

    /// Waits until the log holds `count` request lines and returns them.
    pub async fn log_lines(&self, count: usize) -> Vec<Value> {
        self.event_lines("proxy.request", count).await
    }

    /// Waits until the log holds `count` lines of `event` and returns them.
    pub async fn event_lines(&self, event: &str, count: usize) -> Vec<Value> {
        for _ in 0..200 {
            let lines: Vec<Value> = self
                .read_log()
                .into_iter()
                .filter(|line| line["event"] == event)
                .collect();
            if lines.len() >= count {
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the log never reached {count} `{event}` lines");
    }

    pub fn read_log(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
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

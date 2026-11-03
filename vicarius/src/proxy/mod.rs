// SPDX-License-Identifier: MPL-2.0
//! Serves the local endpoint Claude Code uses as its base URL and routes each request.

mod handler;
mod headers;
mod local;
mod record;
mod tap;
mod usage;

use crate::gate::{Gate, unix_now};
use crate::refresh::Refresher;
use capsa_core::hint::Hint;
use handler::State;
use http_body_util::combinators::UnsyncBoxBody;
use hyper::body::Bytes;
use hyper::http::HeaderValue;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::graceful::GracefulShutdown;
use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

pub use record::{Counts, Route, RouteCounts};
pub use usage::Usage;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type ResBody = UnsyncBoxBody<Bytes, BoxError>;

const ANTHROPIC_URL: &str = "https://api.anthropic.com";
const DEEPSEEK_URL: &str = "https://api.deepseek.com/anthropic";

/// How long a running response may delay shutdown.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// How long the proxy waits for an upstream to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an upstream may stay silent in the middle of a response.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Holds the DeepSeek API key and never shows it: `Debug` prints a placeholder.
#[derive(Clone)]
pub struct DeepseekKey(String);

impl DeepseekKey {
    /// Wraps `key`, or returns `None` when it is empty after trimming.
    pub fn new(key: &str) -> Option<Self> {
        let key = key.trim();
        (!key.is_empty()).then(|| DeepseekKey(key.to_owned()))
    }
}

impl DeepseekKey {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DeepseekKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeepseekKey(<redacted>)")
    }
}

/// Configures one proxy: where it listens and where it sends requests.
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// The port on `127.0.0.1`. Port 0 picks a free one.
    pub port: u16,
    /// The base URL of the Anthropic API, without a trailing slash.
    pub anthropic_base: String,
    /// The base URL of DeepSeek's Anthropic-compatible API, without a trailing slash.
    pub deepseek_base: String,
    pub deepseek_key: Option<DeepseekKey>,
    /// The `proxy.jsonl` file.
    pub log_file: PathBuf,
}

impl ProxyConfig {
    /// Returns the production setup for `port`.
    pub fn new(port: u16, deepseek_key: Option<DeepseekKey>) -> Self {
        ProxyConfig {
            port,
            anthropic_base: ANTHROPIC_URL.to_owned(),
            deepseek_base: DEEPSEEK_URL.to_owned(),
            deepseek_key,
            log_file: capsa_core::paths::state_dir("vicarius").join("proxy.jsonl"),
        }
    }
}

/// Lists what can stop the proxy from starting.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("cannot listen on 127.0.0.1:{port}")]
    Bind {
        port: u16,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot build the upstream HTTP client")]
    Client(#[source] reqwest::Error),
    #[error("the DeepSeek key cannot be sent as a header")]
    KeyHeader,
}

impl Hint for ProxyError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        match self {
            ProxyError::Bind { .. } => Some(Cow::Borrowed(
                "another program uses the port; choose a free one with `vicarius self config set port <n>`",
            )),
            ProxyError::KeyHeader => Some(Cow::Borrowed(
                "check that DEEPSEEK_API_KEY holds only the key",
            )),
            ProxyError::Client(_) => None,
        }
    }
}

/// A proxy that is bound to its port and not yet serving.
pub struct Proxy {
    listener: TcpListener,
    state: Arc<State>,
}

impl Proxy {
    /// Binds `127.0.0.1` on the configured port. No other interface is ever used.
    ///
    /// # Errors
    /// Fails when the port is taken, the client cannot be built or the key is not a valid
    /// header value.
    pub async fn bind(config: ProxyConfig, gate: Arc<Gate>) -> Result<Proxy, ProxyError> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, config.port));
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|source| ProxyError::Bind {
                port: config.port,
                source,
            })?;
        // No decompression features are enabled, so bodies and `content-encoding` pass through
        // byte for byte; `no_proxy` keeps the user's tokens off any system proxy.
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(ProxyError::Client)?;
        let key = match &config.deepseek_key {
            Some(key) => {
                let mut value = HeaderValue::from_str(&key.0).map_err(|_| ProxyError::KeyHeader)?;
                value.set_sensitive(true);
                Some(value)
            }
            None => None,
        };
        let port = listener.local_addr().map_or(config.port, |a| a.port());
        let state = State {
            hosts: [format!("127.0.0.1:{port}"), format!("localhost:{port}")],
            client,
            anthropic_base: config.anthropic_base.trim_end_matches('/').to_owned(),
            deepseek_base: config.deepseek_base.trim_end_matches('/').to_owned(),
            key,
            gate,
            recorder: Arc::new(record::Recorder::new(config.log_file)),
            started_at: unix_now(),
            refresher: None,
        };
        Ok(Proxy {
            listener,
            state: Arc::new(state),
        })
    }

    /// Connects the refresher that `POST /vicarius/refresh` runs and the status reports.
    #[must_use]
    pub fn with_refresher(mut self, refresher: Arc<Refresher>) -> Proxy {
        if let Some(state) = Arc::get_mut(&mut self.state) {
            state.refresher = Some(refresher);
        }
        self
    }

    /// Returns the address the proxy listens on.
    ///
    /// # Errors
    /// Fails when the socket cannot report its address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serves until `shutdown` completes, then lets running responses finish for a few seconds.
    pub async fn run(self, shutdown: impl Future<Output = ()>) {
        let graceful = GracefulShutdown::new();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    let Ok((stream, _)) = accepted else {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    };
                    let state = self.state.clone();
                    let service = service_fn(move |req| handler::handle(state.clone(), req));
                    let conn = http1::Builder::new().serve_connection(TokioIo::new(stream), service);
                    let watched = graceful.watch(conn);
                    tokio::spawn(async move {
                        let _ = watched.await;
                    });
                }
                () = &mut shutdown => break,
            }
        }
        drop(self.listener);
        let _ = tokio::time::timeout(SHUTDOWN_GRACE, graceful.shutdown()).await;
    }
}

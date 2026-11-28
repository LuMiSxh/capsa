// SPDX-License-Identifier: MPL-2.0
//! Serves the local endpoint Claude Code uses as its base URL and routes each request.

mod compat;
mod handler;
mod headers;
mod inline_tools;
mod local;
mod record;
mod tap;
mod usage;

use crate::config::{Auth, Compat};
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

pub use record::{Counts, LOG_LIMIT, Route, RouteCounts};
pub use usage::Usage;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type ResBody = UnsyncBoxBody<Bytes, BoxError>;

const ANTHROPIC_URL: &str = "https://api.anthropic.com";

/// How long a running response may delay shutdown.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// How long the proxy waits for an upstream to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an upstream may stay silent in the middle of a response.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Identifies a configured provider by its name in `config.toml`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderId(String);

impl ProviderId {
    /// Wraps the provider name that its `[providers.<name>]` table gives it.
    pub fn new(name: impl Into<String>) -> Self {
        ProviderId(name.into())
    }

    /// Returns the provider name used in the routes and the status JSON.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Holds a provider API key and never shows it: `Debug` prints a placeholder.
#[derive(Clone)]
pub struct ProviderKey(String);

impl ProviderKey {
    /// Wraps `key`, or returns `None` when it is empty after trimming.
    pub fn new(key: &str) -> Option<Self> {
        let key = key.trim();
        (!key.is_empty()).then(|| ProviderKey(key.to_owned()))
    }
}

impl ProviderKey {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderKey(<redacted>)")
    }
}

/// Describes one provider before the proxy formats its key.
#[derive(Clone, Debug)]
pub struct ProviderSpec {
    pub id: ProviderId,
    pub prefixes: Vec<String>,
    /// The base URL, without a trailing slash.
    pub base_url: String,
    pub key_env: String,
    pub auth: Auth,
    pub models: Vec<String>,
    pub gate: Option<Arc<Gate>>,
    pub compat: Compat,
    pub key: Option<ProviderKey>,
}

/// A provider with its credential ready to send.
pub struct ProviderRuntime {
    pub id: ProviderId,
    pub prefixes: Vec<String>,
    pub base_url: String,
    pub key_env: String,
    pub auth: Auth,
    pub models: Vec<String>,
    pub gate: Option<Arc<Gate>>,
    pub compat: Compat,
    credential: Option<HeaderValue>,
    pub(crate) refresher: Option<Arc<Refresher>>,
}

impl ProviderRuntime {
    /// Formats the provider's key for its auth mode.
    fn resolve(spec: ProviderSpec) -> Result<Self, ProxyError> {
        let credential = match &spec.key {
            Some(key) => {
                let text = match spec.auth {
                    Auth::XApiKey => key.as_str().to_owned(),
                    Auth::Bearer => format!("Bearer {}", key.as_str()),
                };
                let error = || ProxyError::KeyHeader {
                    provider: spec.id.as_str().to_owned(),
                    key_env: spec.key_env.clone(),
                };
                let mut value = HeaderValue::from_str(&text).map_err(|_| error())?;
                value.set_sensitive(true);
                Some(value)
            }
            None => None,
        };
        Ok(ProviderRuntime {
            id: spec.id,
            prefixes: spec.prefixes,
            base_url: spec.base_url.trim_end_matches('/').to_owned(),
            key_env: spec.key_env,
            auth: spec.auth,
            models: spec.models,
            gate: spec.gate,
            compat: spec.compat,
            credential,
            refresher: None,
        })
    }

    /// Returns the credential to send, in the header that `auth` names.
    pub(crate) fn credential(&self) -> Option<&HeaderValue> {
        self.credential.as_ref()
    }
}

/// Configures one proxy: where it listens and where it sends requests.
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// The port on `127.0.0.1`. Port 0 picks a free one.
    pub port: u16,
    /// The base URL of the Anthropic API, without a trailing slash.
    pub anthropic_base: String,
    /// The providers that requests are routed to.
    pub providers: Vec<ProviderSpec>,
    /// The `proxy.jsonl` file.
    pub log_file: PathBuf,
}

impl ProxyConfig {
    /// Returns the production Anthropic URL and log file for `port`.
    pub fn new(port: u16, providers: Vec<ProviderSpec>) -> Self {
        ProxyConfig {
            port,
            anthropic_base: ANTHROPIC_URL.to_owned(),
            providers,
            log_file: crate::proxy_log_file(),
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
    #[error("the `{key_env}` key of provider `{provider}` cannot be sent as a header")]
    KeyHeader { provider: String, key_env: String },
}

impl Hint for ProxyError {
    fn hint(&self) -> Option<Cow<'_, str>> {
        match self {
            ProxyError::Bind { .. } => Some(Cow::Borrowed(
                "another program uses the port; choose a free one with `vicarius self config set port <n>`",
            )),
            ProxyError::KeyHeader { key_env, .. } => Some(Cow::Owned(format!(
                "check that {key_env} holds only the key"
            ))),
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
    /// Fails when the port is taken, the client cannot be built or a key is not a valid header
    /// value.
    pub async fn bind(config: ProxyConfig) -> Result<Proxy, ProxyError> {
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
        let providers = config
            .providers
            .into_iter()
            .map(ProviderRuntime::resolve)
            .collect::<Result<Vec<_>, _>>()?;
        let port = listener.local_addr().map_or(config.port, |a| a.port());
        let routes: Vec<String> = std::iter::once("anthropic".to_owned())
            .chain(providers.iter().map(|p| p.id.as_str().to_owned()))
            .collect();
        let state = State {
            hosts: [format!("127.0.0.1:{port}"), format!("localhost:{port}")],
            client,
            anthropic_base: config.anthropic_base.trim_end_matches('/').to_owned(),
            providers,
            recorder: Arc::new(record::Recorder::new(config.log_file, routes)),
            started_at: unix_now(),
        };
        Ok(Proxy {
            listener,
            state: Arc::new(state),
        })
    }

    /// Connects the refresher that `POST /vicarius/refresh` runs and the status reports.
    #[must_use]
    pub fn with_refresher(mut self, provider: &ProviderId, refresher: Arc<Refresher>) -> Proxy {
        if let Some(state) = Arc::get_mut(&mut self.state)
            && let Some(found) = state.providers.iter_mut().find(|p| &p.id == provider)
        {
            found.refresher = Some(refresher);
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

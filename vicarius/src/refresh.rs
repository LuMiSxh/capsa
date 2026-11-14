// SPDX-License-Identifier: MPL-2.0
//! Keeps the gate in step with the DeepSeek balance and the derived agent files in step with the
//! gate.

use crate::agents::{SyncReport, sync};
use crate::balance::{Availability, decide, parse};
use crate::bulletin::{self, Publisher};
use crate::config::Config;
use crate::gate::{Closed, Gate, unix_now};
use crate::proxy::DeepseekKey;
use capsa_core::log::{self, Level};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinHandle;

/// The base URL of the DeepSeek API, which serves `/user/balance`.
pub const DEEPSEEK_API_URL: &str = "https://api.deepseek.com";

/// The size past which the log rotates.
pub(crate) const LOG_LIMIT: u64 = 8 * 1024 * 1024;

/// How long one balance request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The shortest wait between two checks of the refresh schedule.
const MIN_WAIT: Duration = Duration::from_millis(200);

/// How long a successful refresh keeps a second call from asking DeepSeek again.
const COALESCE_WINDOW: Duration = Duration::from_secs(2);

/// Configures a [`Refresher`].
#[derive(Clone, Debug)]
pub struct RefreshConfig {
    /// The base URL that serves `/user/balance`, without a trailing slash.
    pub balance_base: String,
    /// The DeepSeek key. Without one nothing is fetched and the agents are withdrawn.
    pub key: Option<DeepseekKey>,
    /// The `config.toml` that is read again on every refresh and sync.
    pub config_file: PathBuf,
    /// The directory that holds the agent files.
    pub agents_dir: PathBuf,
    /// The log file, shared with the proxy.
    pub log_file: PathBuf,
    /// The directory that holds the capsa bulletin, where the gate is published.
    pub bulletin_dir: PathBuf,
    /// The time between two agent syncs.
    pub sync_every: Duration,
}

impl RefreshConfig {
    /// Returns the production setup: the real API, the user's config and the Claude agents.
    pub fn new(key: Option<DeepseekKey>) -> Self {
        RefreshConfig {
            balance_base: DEEPSEEK_API_URL.to_owned(),
            key,
            config_file: Config::path(),
            agents_dir: claude_agents_dir(),
            log_file: crate::proxy_log_file(),
            bulletin_dir: capsa_core::bulletin::dir(),
            sync_every: Duration::from_secs(60),
        }
    }
}

/// Returns `$CLAUDE_CONFIG_DIR/agents`, or `~/.claude/agents` when the variable is unset.
pub fn claude_agents_dir() -> PathBuf {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || std::env::home_dir().unwrap_or_default().join(".claude"),
            PathBuf::from,
        );
    base.join("agents")
}

/// The refresh half of the status JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RefreshView {
    /// Unix seconds of the last answer from DeepSeek.
    pub last_ok_at: Option<u64>,
    /// The last failure, cleared by the next answer.
    pub last_error: Option<String>,
    /// Unix seconds of the next scheduled refresh.
    pub next_at: Option<u64>,
}

/// Lists what can stop a refresher from being built.
#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    #[error("cannot build the HTTP client for the balance")]
    Client(#[source] reqwest::Error),
    #[error("the DeepSeek key cannot be sent as a header")]
    KeyHeader,
}

#[derive(Default)]
struct Inner {
    view: RefreshView,
    report: Option<SyncReport>,
    /// Counts finished balance requests, including failed ones.
    completed: u64,
    /// The start of the last successful balance request and the gate generation it saw.
    last_success: Option<LastSuccess>,
}

#[derive(Clone, Copy)]
struct LastSuccess {
    at: Instant,
    generation: u64,
}

/// What one balance request found.
enum Fetched {
    Decision(Availability),
    KeyRejected,
    PaymentRequired,
}

#[derive(Serialize)]
struct Line<'a> {
    outcome: &'static str,
    amount: Option<f64>,
    currency: Option<&'a str>,
    error: Option<&'a str>,
}

/// Reads the balance, sets the gate and syncs the agent files.
pub struct Refresher {
    gate: Arc<Gate>,
    client: reqwest::Client,
    url: String,
    bearer: Option<HeaderValue>,
    config_file: PathBuf,
    agents_dir: PathBuf,
    log_file: PathBuf,
    sync_every: Duration,
    publisher: Publisher,
    config: Mutex<Config>,
    inner: Mutex<Inner>,
    refresh_lock: AsyncMutex<()>,
}

impl Refresher {
    /// Builds a refresher for `gate`. Nothing runs until [`Refresher::run`] or a refresh call.
    ///
    /// # Errors
    /// Fails when the client cannot be built or the key is not a valid header value.
    pub fn new(config: RefreshConfig, gate: Arc<Gate>) -> Result<Refresher, RefreshError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(RefreshError::Client)?;
        let bearer = match &config.key {
            Some(key) => {
                let mut value = HeaderValue::from_str(&format!("Bearer {}", key.as_str()))
                    .map_err(|_| RefreshError::KeyHeader)?;
                value.set_sensitive(true);
                Some(value)
            }
            None => None,
        };
        let initial = Config::load_from(&config.config_file).unwrap_or_default();
        let publisher = Publisher::new(config.bulletin_dir, config.log_file.clone(), gate.clone());
        Ok(Refresher {
            gate,
            client,
            url: format!("{}/user/balance", config.balance_base.trim_end_matches('/')),
            bearer,
            config_file: config.config_file,
            agents_dir: config.agents_dir,
            log_file: config.log_file,
            sync_every: config.sync_every,
            publisher,
            config: Mutex::new(initial),
            inner: Mutex::new(Inner::default()),
            refresh_lock: AsyncMutex::new(()),
        })
    }

    /// Writes the gate to the capsa bulletin in the background, with the TTL of the config last read.
    ///
    /// The returned handle may be dropped; the write finishes on its own.
    pub fn publish_gate(&self) -> JoinHandle<()> {
        let minutes = self
            .config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .balance_refresh_minutes;
        self.publisher.spawn(bulletin::ttl_secs(minutes))
    }

    /// Returns the last sync report and the refresh timestamps.
    pub fn view(&self) -> (Option<SyncReport>, RefreshView) {
        let inner = self.inner();
        (inner.report.clone(), inner.view.clone())
    }

    /// Reads the balance once and sets the gate. Without a key it does nothing.
    ///
    /// A call that overlaps a running refresh, or repeats a successful one within
    /// [`COALESCE_WINDOW`] on the same gate generation, waits for that refresh and returns.
    /// `vicarius self start` asks for a refresh right after launching the service, whose own
    /// startup refresh usually still runs; both would otherwise read the same balance twice.
    pub async fn refresh(&self) {
        let Some(bearer) = &self.bearer else {
            return;
        };
        let seen = self.inner().completed;
        let _running = self.refresh_lock.lock().await;
        let generation = self.gate.generation();
        {
            let inner = self.inner();
            let recent = inner.last_success.is_some_and(|last| {
                last.generation == generation && last.at.elapsed() < COALESCE_WINDOW
            });
            if inner.completed != seen || recent {
                return;
            }
        }
        let started = Instant::now();
        let config = self.current_config();
        let result = self.fetch(bearer, config.min_balance_usd).await;
        let now = unix_now();
        let next = now + u64::from(config.balance_refresh_minutes) * 60;
        match &result {
            Ok(fetched) => self.apply(fetched, generation, now),
            Err(error) => self.record_error(error),
        }
        if result.is_ok() {
            let _ = self.publish_gate().await;
        }
        let mut inner = self.inner();
        inner.completed += 1;
        inner.view.next_at = Some(next);
        if result.is_ok() {
            inner.last_success = Some(LastSuccess {
                at: started,
                generation,
            });
            inner.view.last_ok_at = Some(now);
            inner.view.last_error = None;
        }
    }

    /// Syncs the agent files with the gate, reading `config.toml` again.
    pub fn sync_agents(&self) {
        let available = self.gate.closed().is_none();
        self.sync_with(available);
    }

    /// Refreshes the balance, then syncs the agent files.
    pub async fn refresh_and_sync(&self) {
        self.refresh().await;
        self.sync_agents();
    }

    /// Removes every derived agent file.
    pub fn withdraw_agents(&self) {
        self.sync_with(false);
    }

    /// Refreshes at start and on schedule, syncs on a timer, and withdraws the agents once
    /// `stop` turns true or its sender is gone.
    pub async fn run(self: Arc<Self>, mut stop: watch::Receiver<bool>) {
        self.refresh_and_sync().await;
        let mut tick = tokio::time::interval(self.sync_every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;
        loop {
            tokio::select! {
                () = tokio::time::sleep(self.until_next()) => {
                    if self.is_due() {
                        self.refresh_and_sync().await;
                    }
                }
                _ = tick.tick() => self.sync_agents(),
                () = async { let _ = stop.wait_for(|stopped| *stopped).await; } => break,
            }
        }
        self.withdraw_agents();
    }

    fn sync_with(&self, available: bool) {
        let config = self.current_config();
        let report = sync(&self.agents_dir, &config.agents, available);
        self.inner().report = Some(report);
    }

    fn current_config(&self) -> Config {
        let mut kept = self.config.lock().unwrap_or_else(PoisonError::into_inner);
        if let Ok(fresh) = Config::load_from(&self.config_file) {
            *kept = fresh;
        }
        kept.clone()
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn until_next(&self) -> Duration {
        match self.inner().view.next_at {
            Some(at) => Duration::from_secs(at.saturating_sub(unix_now())).max(MIN_WAIT),
            None => self.sync_every,
        }
    }

    fn is_due(&self) -> bool {
        self.inner().view.next_at.is_some_and(|at| at <= unix_now())
    }

    async fn fetch(&self, bearer: &HeaderValue, min_usd: f64) -> Result<Fetched, String> {
        let response = self
            .client
            .get(&self.url)
            .header(AUTHORIZATION, bearer.clone())
            .send()
            .await
            .map_err(|e| format!("balance request failed: {e}"))?;
        match response.status().as_u16() {
            401 => return Ok(Fetched::KeyRejected),
            402 => return Ok(Fetched::PaymentRequired),
            200..=299 => {}
            other => return Err(format!("balance request answered HTTP {other}")),
        }
        let body = response
            .bytes()
            .await
            .map_err(|e| format!("balance answer cut off: {e}"))?;
        let balance = parse(&body).map_err(|e| e.to_string())?;
        Ok(Fetched::Decision(decide(&balance, min_usd)))
    }

    fn apply(&self, fetched: &Fetched, generation: u64, now: u64) {
        let (outcome, amount, currency) = match fetched {
            Fetched::KeyRejected => {
                self.gate.close(Closed::KeyRejected, now);
                ("key_rejected", None, None)
            }
            Fetched::PaymentRequired => {
                self.gate.close(Closed::PaymentRequired, now);
                ("payment_required", None, None)
            }
            Fetched::Decision(Availability::Unavailable) => {
                self.gate.close(Closed::Unavailable, now);
                ("unavailable", None, None)
            }
            Fetched::Decision(Availability::Available { amount, currency }) => {
                self.gate
                    .open_if_unchanged(generation, *amount, currency, now);
                ("available", Some(*amount), Some(currency.as_str()))
            }
            Fetched::Decision(Availability::Low {
                amount,
                currency,
                min,
            }) => {
                self.gate.note_balance(*amount, currency, now);
                self.gate.close(Closed::LowBalance { minimum: *min }, now);
                ("low", Some(*amount), Some(currency.as_str()))
            }
        };
        self.write_log(
            Level::Info,
            &Line {
                outcome,
                amount,
                currency,
                error: None,
            },
        );
    }

    fn record_error(&self, error: &str) {
        self.inner().view.last_error = Some(error.to_owned());
        let line = Line {
            outcome: "error",
            amount: None,
            currency: None,
            error: Some(error),
        };
        self.write_log(Level::Warn, &line);
    }

    fn write_log(&self, level: Level, line: &Line<'_>) {
        if let Some(dir) = self.log_file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = log::append(&self.log_file, LOG_LIMIT, level, "balance.refresh", line);
    }
}

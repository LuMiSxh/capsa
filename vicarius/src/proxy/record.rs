// SPDX-License-Identifier: MPL-2.0
//! Counts requests and tokens per route and writes one log line per request.

use super::ProviderId;
use super::headers::HeaderLog;
use super::usage::Usage;
use capsa_core::log::{self, Level};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// The size past which `proxy.jsonl` rotates to `proxy.jsonl.old`.
pub const LOG_LIMIT: u64 = 8 * 1024 * 1024;

/// Names where a request went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Anthropic,
    Provider(ProviderId),
}

impl Route {
    /// Returns the route name used in the log and the status JSON.
    pub fn name(&self) -> &str {
        match self {
            Route::Anthropic => "anthropic",
            Route::Provider(id) => id.as_str(),
        }
    }
}

/// The fields of a `proxy.request` log line.
#[derive(Serialize)]
pub struct RequestLine {
    pub route: String,
    pub model: Option<String>,
    pub method: String,
    pub path: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_status: Option<u16>,
    #[serde(flatten)]
    pub usage: Usage,
    pub duration_ms: u64,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub headers: Vec<String>,
    pub header_values: BTreeMap<String, String>,
}

/// The `route` value of a request that no route handled.
const ROUTE_NONE: &str = "none";

impl RequestLine {
    pub fn new(
        route: &Route,
        model: Option<String>,
        method: &str,
        path: &str,
        h: HeaderLog,
    ) -> Self {
        Self::with_route(route.name(), model, method, path, h)
    }

    /// Builds the line of a request refused before routing.
    pub fn forbidden(method: &str, path: &str, h: HeaderLog) -> Self {
        let mut line = Self::with_route(ROUTE_NONE, None, method, path, h);
        line.status = 403;
        line
    }

    fn with_route(
        route: &str,
        model: Option<String>,
        method: &str,
        path: &str,
        h: HeaderLog,
    ) -> Self {
        RequestLine {
            route: route.to_owned(),
            model,
            method: method.to_owned(),
            path: path.to_owned(),
            status: 0,
            upstream_status: None,
            usage: Usage::default(),
            duration_ms: 0,
            session_id: h.session_id,
            agent_id: h.agent_id,
            headers: h.names,
            header_values: h.values,
        }
    }
}

/// The counters of one route since the proxy started.
#[derive(Default)]
struct RouteCounters {
    requests: AtomicU64,
    refused: AtomicU64,
    input: AtomicU64,
    cache_creation: AtomicU64,
    cache_read: AtomicU64,
    output: AtomicU64,
}

/// A copy of one route's counters, as the status endpoint shows them.
#[derive(Serialize)]
pub struct RouteCounts {
    pub requests: u64,
    pub refused: u64,
    pub input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
}

/// A copy of all counters.
#[derive(Serialize)]
pub struct Counts {
    /// One entry per route, `anthropic` included, in name order.
    pub routes: BTreeMap<String, RouteCounts>,
    /// Requests refused for a foreign `Host` or an `Origin` header, on any path.
    pub forbidden: u64,
}

/// Owns the request counters and the log file.
pub struct Recorder {
    file: PathBuf,
    write: Mutex<()>,
    routes: BTreeMap<String, RouteCounters>,
    forbidden: AtomicU64,
    log_failing: AtomicBool,
}

impl Recorder {
    /// Builds a recorder with one zeroed counter set per route name.
    pub fn new(file: PathBuf, routes: impl IntoIterator<Item = String>) -> Self {
        Recorder {
            file,
            write: Mutex::new(()),
            routes: routes
                .into_iter()
                .map(|name| (name, RouteCounters::default()))
                .collect(),
            forbidden: AtomicU64::new(0),
            log_failing: AtomicBool::new(false),
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "the recorder is built from the same provider table that builds every route"
    )]
    fn counters(&self, route: &Route) -> &RouteCounters {
        self.routes
            .get(route.name())
            .expect("every route has a counter set")
    }

    /// Counts a request when it is routed, before its response ends.
    pub fn begin(&self, route: &Route) {
        self.counters(route)
            .requests
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Counts a request refused by the closed gate.
    pub fn refuse(&self, route: &Route) {
        self.counters(route).refused.fetch_add(1, Ordering::Relaxed);
    }

    /// Adds the tokens of a finished request and writes its log line.
    pub fn finish(&self, route: &Route, line: &RequestLine) {
        let c = self.counters(route);
        let add = |slot: &AtomicU64, n: u64| slot.fetch_add(n, Ordering::Relaxed);
        add(&c.input, line.usage.input_tokens);
        add(&c.cache_creation, line.usage.cache_creation_tokens);
        add(&c.cache_read, line.usage.cache_read_tokens);
        add(&c.output, line.usage.output_tokens);
        self.write_line(line);
    }

    /// Counts a request refused for its `Host` or `Origin` and writes its log line.
    pub fn forbid(&self, line: &RequestLine) {
        self.forbidden.fetch_add(1, Ordering::Relaxed);
        self.write_line(line);
    }

    /// Writes the `unknown_model` line of a model id that no provider prefix matched.
    pub fn unknown_model(&self, model: &str) {
        self.append(Level::Warn, "unknown_model", &UnknownModel { model });
    }

    fn write_line(&self, line: &RequestLine) {
        let level = if line.status >= 400 {
            Level::Warn
        } else {
            Level::Info
        };
        self.append(level, "proxy.request", line);
    }

    fn append<T: Serialize>(&self, level: Level, event: &str, line: &T) {
        let _guard = self
            .write
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let written = self
            .file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| log::append(&self.file, LOG_LIMIT, level, event, line));
        self.report_log_failure(written.err());
    }

    /// Names a failing log write on stderr once per run of failures, so a full disk or a bad
    /// path does not go unseen and does not flood stderr either. The request is never failed.
    fn report_log_failure(&self, error: Option<std::io::Error>) {
        let Some(error) = error else {
            self.log_failing.store(false, Ordering::Relaxed);
            return;
        };
        if !self.log_failing.swap(true, Ordering::Relaxed) {
            let _ = writeln!(
                std::io::stderr(),
                "vicarius: cannot write {}: {error}",
                self.file.display()
            );
        }
    }

    /// Returns a copy of the counters.
    pub fn counts(&self) -> Counts {
        let copy = |c: &RouteCounters| RouteCounts {
            requests: c.requests.load(Ordering::Relaxed),
            refused: c.refused.load(Ordering::Relaxed),
            input_tokens: c.input.load(Ordering::Relaxed),
            cache_creation_tokens: c.cache_creation.load(Ordering::Relaxed),
            cache_read_tokens: c.cache_read.load(Ordering::Relaxed),
            output_tokens: c.output.load(Ordering::Relaxed),
        };
        let routes = self
            .routes
            .iter()
            .map(|(name, counters)| (name.clone(), copy(counters)))
            .collect();
        Counts {
            routes,
            forbidden: self.forbidden.load(Ordering::Relaxed),
        }
    }
}

/// The fields of an `unknown_model` log line.
#[derive(Serialize)]
struct UnknownModel<'a> {
    model: &'a str,
}

/// A request whose response is still passing; logs itself when dropped.
pub struct Pending {
    recorder: std::sync::Arc<Recorder>,
    route: Route,
    line: RequestLine,
    started: Instant,
}

impl Pending {
    pub fn new(
        recorder: std::sync::Arc<Recorder>,
        route: Route,
        line: RequestLine,
        started: Instant,
    ) -> Self {
        Pending {
            recorder,
            route,
            line,
            started,
        }
    }

    /// Writes the line with the usage read from the response.
    pub fn finish(mut self, usage: Usage) {
        self.line.usage = usage;
        self.line.duration_ms =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.recorder.finish(&self.route, &self.line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_model_line_carries_the_id_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proxy.jsonl");
        let recorder = Recorder::new(path.clone(), ["anthropic".to_owned()]);
        recorder.unknown_model("deepsek-flash");
        let line: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
        assert_eq!(line["event"], "unknown_model");
        assert_eq!(line["level"], "warn");
        assert_eq!(line["model"], "deepsek-flash");
        assert_eq!(line.as_object().unwrap().len(), 4, "{line}");
    }

    #[test]
    fn every_route_starts_at_zero_and_the_counts_keep_one_entry_each() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::new(
            dir.path().join("proxy.jsonl"),
            ["anthropic".to_owned(), "deepseek".to_owned()],
        );
        recorder.begin(&Route::Provider(ProviderId::new("deepseek")));
        let counts = recorder.counts();
        assert_eq!(counts.routes.len(), 2);
        assert_eq!(counts.routes["anthropic"].requests, 0);
        assert_eq!(counts.routes["deepseek"].requests, 1);
        assert_eq!(counts.forbidden, 0);
    }
}

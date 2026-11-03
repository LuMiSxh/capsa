// SPDX-License-Identifier: MPL-2.0
//! Answers the proxy's own endpoints under `/vicarius/`, which never collide with `/v1`.

use super::ResBody;
use super::handler::{State, full};
use super::record::Counts;
use crate::agents::SyncReport;
use crate::refresh::RefreshView;
use hyper::http::request::Parts;
use hyper::{Method, Response, StatusCode};
use serde::Serialize;
use serde_json::json;

#[derive(Serialize)]
struct Status {
    started_at: u64,
    gate: GateView,
    counters: Counts,
    agents: Option<SyncReport>,
    refresh: RefreshView,
}

#[derive(Serialize)]
struct GateView {
    open: bool,
    reason: Option<&'static str>,
    reason_text: Option<String>,
    balance: Option<f64>,
    currency: Option<String>,
    checked_at: Option<u64>,
}

/// Answers a request for a path under `/vicarius/`.
pub async fn respond(state: &State, parts: &Parts) -> Response<ResBody> {
    match (&parts.method, parts.uri.path()) {
        (&Method::GET, "/vicarius/status") => status(state),
        (&Method::POST, "/vicarius/refresh") => {
            if let Some(refresher) = &state.refresher {
                refresher.refresh_and_sync().await;
            }
            status(state)
        }
        _ => {
            let body = json!({"type": "error", "error": {"type": "not_found_error", "message": "unknown vicarius endpoint"}});
            json_response(StatusCode::NOT_FOUND, &body)
        }
    }
}

fn status(state: &State) -> Response<ResBody> {
    let gate = state.gate.snapshot();
    let (agents, refresh) = state
        .refresher
        .as_ref()
        .map_or_else(|| (None, RefreshView::default()), |r| r.view());
    let status = Status {
        started_at: state.started_at,
        gate: GateView {
            open: gate.is_open(),
            reason: gate.closed.as_ref().map(crate::gate::Closed::code),
            reason_text: gate.closed.as_ref().map(ToString::to_string),
            balance: gate.balance,
            currency: gate.currency,
            checked_at: gate.checked_at,
        },
        counters: state.recorder.counts(),
        agents,
        refresh,
    };
    json_response(StatusCode::OK, &status)
}

/// Builds a JSON response from any serializable value.
pub fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response<ResBody> {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    full(status, "application/json", bytes)
}

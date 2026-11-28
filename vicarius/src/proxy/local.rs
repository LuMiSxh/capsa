// SPDX-License-Identifier: MPL-2.0
//! Answers the proxy's own endpoints under `/vicarius/`, which never collide with `/v1`.

use std::collections::BTreeMap;

use super::ProviderRuntime;
use super::ResBody;
use super::handler::{State, full};
use super::record::Counts;
use crate::agents::SyncReport;
use crate::gate::Gate;
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
    providers: BTreeMap<String, ProviderView>,
}

#[derive(Serialize)]
struct ProviderView {
    gated: bool,
    gate: GateView,
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
            if let Some(refresher) = deepseek(state).and_then(|p| p.refresher.as_ref()) {
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

fn deepseek(state: &State) -> Option<&ProviderRuntime> {
    state
        .providers
        .iter()
        .find(|provider| provider.id.as_str() == "deepseek")
}

fn status(state: &State) -> Response<ResBody> {
    let providers = state
        .providers
        .iter()
        .map(|provider| {
            let view = ProviderView {
                gated: provider.gate.is_some(),
                gate: gate_view(provider.gate.as_deref()),
                refresh: provider
                    .refresher
                    .as_ref()
                    .map_or_else(RefreshView::default, |r| r.view().1),
            };
            (provider.id.as_str().to_owned(), view)
        })
        .collect();
    let provider = deepseek(state);
    let (agents, refresh) = provider
        .and_then(|provider| provider.refresher.as_ref())
        .map_or_else(|| (None, RefreshView::default()), |r| r.view());
    let status = Status {
        started_at: state.started_at,
        gate: gate_view(provider.and_then(|provider| provider.gate.as_deref())),
        counters: state.recorder.counts(),
        agents,
        refresh,
        providers,
    };
    json_response(StatusCode::OK, &status)
}

fn gate_view(gate: Option<&Gate>) -> GateView {
    gate.map_or_else(
        || GateView {
            open: true,
            reason: None,
            reason_text: None,
            balance: None,
            currency: None,
            checked_at: None,
        },
        |gate| {
            let snapshot = gate.snapshot();
            GateView {
                open: snapshot.is_open(),
                reason: snapshot.closed.as_ref().map(crate::gate::Closed::code),
                reason_text: snapshot.closed.as_ref().map(ToString::to_string),
                balance: snapshot.balance,
                currency: snapshot.currency,
                checked_at: snapshot.checked_at,
            }
        },
    )
}

/// Builds a JSON response from any serializable value.
pub fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response<ResBody> {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    full(status, "application/json", bytes)
}

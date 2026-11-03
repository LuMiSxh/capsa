// SPDX-License-Identifier: MPL-2.0
//! Routes one request by its model and relays the upstream answer without holding it back.

use super::headers::{HeaderLog, strip_hop_by_hop, strip_request_framing};
use super::record::{Pending, Recorder, RequestLine, Route};
use super::tap::Tap;
use super::usage::{Usage, UsageTap};
use super::{BoxError, ResBody, local};
use crate::gate::{Closed, Gate, unix_now};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::http::request::Parts;
use hyper::http::uri::PathAndQuery;
use hyper::http::{Extensions, HeaderValue, Version};
use hyper::{Request, Response, StatusCode};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, HOST, HeaderName, ORIGIN};
use serde::Deserialize;
use serde_json::json;
use std::borrow::Cow;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

/// The largest request body the proxy reads, in bytes.
const MAX_REQUEST: usize = 256 << 20;

const X_API_KEY: HeaderName = HeaderName::from_static("x-api-key");
const X_SHOULD_RETRY: HeaderName = HeaderName::from_static("x-should-retry");

/// Everything a request needs, shared by all connections.
pub struct State {
    pub client: reqwest::Client,
    pub anthropic_base: String,
    pub deepseek_base: String,
    pub key: Option<HeaderValue>,
    pub gate: Arc<Gate>,
    pub recorder: Arc<Recorder>,
    pub started_at: u64,
    pub refresher: Option<Arc<crate::refresh::Refresher>>,
    /// The `Host` values a request may carry: the loopback names with the bound port.
    pub hosts: [String; 2],
}

#[derive(Deserialize)]
struct ModelOnly<'a> {
    #[serde(borrow)]
    model: Option<Cow<'a, str>>,
}

/// Answers one request. It never fails: every error becomes a response.
pub async fn handle(
    state: Arc<State>,
    req: Request<Incoming>,
) -> Result<Response<ResBody>, Infallible> {
    let (parts, body) = req.into_parts();
    if let Some(why) = foreign(&state, &parts) {
        let line = RequestLine::forbidden(
            parts.method.as_str(),
            parts.uri.path(),
            HeaderLog::from_headers(&parts.headers),
        );
        state.recorder.forbid(&line);
        return Ok(json_error(
            StatusCode::FORBIDDEN,
            "permission_error",
            &format!("vicarius refuses this request ({why})"),
        ));
    }
    let path = parts.uri.path();
    if path == "/vicarius" || path.starts_with("/vicarius/") {
        return Ok(local::respond(&state, &parts).await);
    }
    let body = match Limited::new(body, MAX_REQUEST).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return Ok(json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "vicarius could not read the request body",
            ));
        }
    };
    Ok(route(&state, &parts, body).await)
}

/// Names why a request cannot come from Claude Code, or `None` when it can.
///
/// Claude Code sends `Host: 127.0.0.1:<port>` and no `Origin`; a browser page always sends an
/// `Origin` on a cross-origin `POST`, and a rebound name arrives as a foreign `Host`.
fn foreign(state: &State, parts: &Parts) -> Option<&'static str> {
    if parts.headers.contains_key(ORIGIN) {
        return Some("origin header");
    }
    let mut hosts = parts.headers.get_all(HOST).iter();
    let host_ok = match (hosts.next(), hosts.next()) {
        (Some(host), None) => state.hosts.iter().any(|h| h.as_bytes() == host.as_bytes()),
        _ => false,
    };
    (!host_ok).then_some("host header")
}

fn model_of(parts: &Parts, body: &[u8]) -> Option<String> {
    let parsed = parts.method == hyper::Method::POST
        && matches!(
            parts.uri.path(),
            "/v1/messages" | "/v1/messages/count_tokens"
        );
    if !parsed {
        return None;
    }
    let model = serde_json::from_slice::<ModelOnly>(body).ok()?.model?;
    Some(model.into_owned())
}

async fn route(state: &Arc<State>, parts: &Parts, body: Bytes) -> Response<ResBody> {
    let started = Instant::now();
    let model = model_of(parts, &body);
    let route = if model.as_deref().is_some_and(|m| m.starts_with("deepseek-")) {
        Route::Deepseek
    } else {
        Route::Anthropic
    };
    state.recorder.begin(route);
    let mut line = RequestLine::new(
        route,
        model,
        parts.method.as_str(),
        parts.uri.path(),
        HeaderLog::from_headers(&parts.headers),
    );
    let pending = |line| Pending::new(state.recorder.clone(), route, line, started);
    if route == Route::Deepseek
        && let Some(reason) = refusal(state)
    {
        state.recorder.refuse(route);
        line.status = 400;
        pending(line).finish(Usage::default());
        return closed_response(&reason);
    }
    let Ok(upstream) = send(state, route, parts, body).await else {
        line.status = 502;
        pending(line).finish(Usage::default());
        let host = if route == Route::Deepseek {
            "DeepSeek"
        } else {
            "Anthropic"
        };
        return json_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            &format!("vicarius could not reach {host}"),
        );
    };
    let status = upstream.status();
    if route == Route::Deepseek {
        let reason = match status {
            StatusCode::PAYMENT_REQUIRED => Some(Closed::PaymentRequired),
            StatusCode::UNAUTHORIZED => Some(Closed::KeyRejected),
            _ => None,
        };
        if let Some(reason) = reason {
            state.gate.close(reason.clone(), unix_now());
            state.recorder.refuse(route);
            line.status = 400;
            line.upstream_status = Some(status.as_u16());
            pending(line).finish(Usage::default());
            return closed_response(&reason);
        }
    }
    relay(upstream, line, pending)
}

fn refusal(state: &State) -> Option<Closed> {
    state
        .gate
        .closed()
        .or_else(|| state.key.is_none().then_some(Closed::NoKey))
}

async fn send(
    state: &State,
    route: Route,
    parts: &Parts,
    body: Bytes,
) -> Result<reqwest::Response, reqwest::Error> {
    let base = match route {
        Route::Anthropic => &state.anthropic_base,
        Route::Deepseek => &state.deepseek_base,
    };
    let target = parts.uri.path_and_query().map_or("/", PathAndQuery::as_str);
    let mut headers = parts.headers.clone();
    strip_hop_by_hop(&mut headers);
    strip_request_framing(&mut headers);
    if route == Route::Deepseek {
        headers.remove(AUTHORIZATION);
        headers.remove(X_API_KEY);
        headers.remove(COOKIE);
        if let Some(key) = &state.key {
            headers.insert(X_API_KEY, key.clone());
        }
    }
    let mut request = state
        .client
        .request(parts.method.clone(), format!("{base}{target}"))
        .headers(headers);
    if !body.is_empty() {
        request = request.body(body);
    }
    request.send().await
}

fn relay(
    upstream: reqwest::Response,
    mut line: RequestLine,
    pending: impl FnOnce(RequestLine) -> Pending,
) -> Response<ResBody> {
    line.status = upstream.status().as_u16();
    let response: Response<reqwest::Body> = upstream.into();
    let usage = UsageTap::for_response(response.status(), response.headers());
    let (mut head, body) = response.into_parts();
    strip_hop_by_hop(&mut head.headers);
    head.extensions = Extensions::new();
    head.version = Version::HTTP_11;
    let tap = Tap::new(body, usage, pending(line));
    Response::from_parts(head, tap.map_err(BoxError::from).boxed_unsync())
}

fn closed_response(reason: &Closed) -> Response<ResBody> {
    let message =
        format!("DeepSeek is currently unavailable ({reason}). Use worker or senior-worker.");
    let mut response = json_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
    response
        .headers_mut()
        .insert(X_SHOULD_RETRY, HeaderValue::from_static("false"));
    response
}

/// Builds an Anthropic-shaped error response.
fn json_error(status: StatusCode, kind: &str, message: &str) -> Response<ResBody> {
    let body = json!({"type": "error", "error": {"type": kind, "message": message}});
    local::json_response(status, &body)
}

pub(super) fn full(
    status: StatusCode,
    content_type: &'static str,
    bytes: Vec<u8>,
) -> Response<ResBody> {
    let mut response = Response::new(
        Full::new(Bytes::from(bytes))
            .map_err(|never| match never {})
            .boxed_unsync(),
    );
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

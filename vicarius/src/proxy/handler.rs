// SPDX-License-Identifier: MPL-2.0
//! Routes one request by its model and relays the upstream answer without holding it back.

use super::compat;
use super::headers::{HeaderLog, strip_hop_by_hop, strip_request_framing};
use super::record::{Pending, Recorder, RequestLine, Route};
use super::tap::Tap;
use super::usage::{Usage, UsageTap};
use super::{BoxError, ProviderRuntime, ResBody, local};
use crate::config::Auth;
use crate::gate::{Closed, unix_now};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::http::request::Parts;
use hyper::http::uri::PathAndQuery;
use hyper::http::{Extensions, HeaderValue, Version};
use hyper::{Method, Request, Response, StatusCode};
use reqwest::header::{
    ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE, COOKIE, HOST, HeaderName, ORIGIN,
};
use serde::{Deserialize, Serialize};
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
    pub providers: Vec<ProviderRuntime>,
    pub recorder: Arc<Recorder>,
    pub started_at: u64,
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
    if parts.method == Method::GET && path == "/v1/models" {
        return Ok(models(&state));
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

/// Returns the longest provider prefix that matches `model`, or `None` for Anthropic.
fn provider_of<'a>(state: &'a State, model: &str) -> Option<&'a ProviderRuntime> {
    state
        .providers
        .iter()
        .flat_map(|provider| {
            provider
                .prefixes
                .iter()
                .map(move |prefix| (prefix.as_str(), provider))
        })
        .filter(|(prefix, _)| model.starts_with(*prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, provider)| provider)
}

async fn route(state: &Arc<State>, parts: &Parts, body: Bytes) -> Response<ResBody> {
    let started = Instant::now();
    let model = model_of(parts, &body);
    let provider = match model.as_deref() {
        Some(model) => provider_of(state, model).or_else(|| {
            // Claude models are expected to take the Anthropic route; only an id outside every
            // provider prefix is a config or model typo worth warning about.
            if !model.starts_with("claude") {
                state.recorder.unknown_model(model);
            }
            None
        }),
        None => None,
    };
    let route = match provider {
        Some(provider) => Route::Provider(provider.id.clone()),
        None => Route::Anthropic,
    };
    state.recorder.begin(&route);
    let mut line = RequestLine::new(
        &route,
        model,
        parts.method.as_str(),
        parts.uri.path(),
        HeaderLog::from_headers(&parts.headers),
    );
    if let Some(provider) = provider
        && let Some(why) = refusal(provider)
    {
        state.recorder.refuse(&route);
        line.status = 400;
        Pending::new(state.recorder.clone(), route, line, started).finish(Usage::default());
        return closed_response(&why);
    }
    let Ok(upstream) = send(state, provider, parts, body).await else {
        line.status = 502;
        let host = provider.map_or("Anthropic", |p| p.id.as_str());
        Pending::new(state.recorder.clone(), route, line, started).finish(Usage::default());
        return json_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            &format!("vicarius could not reach {host}"),
        );
    };
    let status = upstream.status();
    if let Some(provider) = provider
        && let Some(reason) = close_reason(status)
    {
        if let Some(gate) = &provider.gate {
            gate.close(reason.clone(), unix_now());
            if let Some(refresher) = &provider.refresher {
                // Detached: the bulletin write finishes after the response has gone out.
                drop(refresher.publish_gate());
            }
        }
        state.recorder.refuse(&route);
        line.status = 400;
        line.upstream_status = Some(status.as_u16());
        let why = format!(
            "provider `{}` is unavailable ({reason}); its key comes from {}",
            provider.id, provider.key_env
        );
        Pending::new(state.recorder.clone(), route, line, started).finish(Usage::default());
        return closed_response(&why);
    }
    let pending = |line| Pending::new(state.recorder.clone(), route, line, started);
    relay(upstream, line, pending)
}

fn close_reason(status: StatusCode) -> Option<Closed> {
    match status {
        StatusCode::PAYMENT_REQUIRED => Some(Closed::PaymentRequired),
        StatusCode::UNAUTHORIZED => Some(Closed::KeyRejected),
        _ => None,
    }
}

/// Names why a provider refuses requests, or `None` when it accepts them.
fn refusal(provider: &ProviderRuntime) -> Option<String> {
    if let Some(gate) = &provider.gate
        && let Some(closed) = gate.closed()
    {
        return Some(format!(
            "provider `{}` is unavailable ({closed}); its key comes from {}",
            provider.id, provider.key_env
        ));
    }
    provider.credential().is_none().then(|| {
        format!(
            "provider `{}` has no key; set {}",
            provider.id, provider.key_env
        )
    })
}

async fn send(
    state: &State,
    provider: Option<&ProviderRuntime>,
    parts: &Parts,
    body: Bytes,
) -> Result<reqwest::Response, reqwest::Error> {
    let base = provider.map_or(state.anthropic_base.as_str(), |p| p.base_url.as_str());
    let target = parts.uri.path_and_query().map_or("/", PathAndQuery::as_str);
    let mut headers = parts.headers.clone();
    strip_hop_by_hop(&mut headers);
    strip_request_framing(&mut headers);
    // The usage tap reads only plain bodies, so the upstream must not compress the response.
    headers.remove(ACCEPT_ENCODING);
    let mut body = body;
    if let Some(provider) = provider {
        headers.remove(AUTHORIZATION);
        headers.remove(X_API_KEY);
        headers.remove(COOKIE);
        if let Some(key) = provider.credential() {
            let name = match provider.auth {
                Auth::XApiKey => X_API_KEY,
                Auth::Bearer => AUTHORIZATION,
            };
            headers.insert(name, key.clone());
        }
        if let Cow::Owned(bytes) = compat::apply(&body, &provider.compat) {
            body = Bytes::from(bytes);
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

fn closed_response(why: &str) -> Response<ResBody> {
    let mut response = json_error(StatusCode::BAD_REQUEST, "invalid_request_error", why);
    response
        .headers_mut()
        .insert(X_SHOULD_RETRY, HeaderValue::from_static("false"));
    response
}

#[derive(Serialize)]
struct ModelList {
    object: &'static str,
    data: Vec<ModelEntry>,
}

#[derive(Serialize)]
struct ModelEntry {
    id: String,
    object: &'static str,
    created: u64,
    owned_by: String,
}

fn models(state: &State) -> Response<ResBody> {
    let data = state
        .providers
        .iter()
        .flat_map(|provider| {
            provider.models.iter().map(|model| ModelEntry {
                id: model.clone(),
                object: "model",
                created: state.started_at,
                owned_by: provider.id.as_str().to_owned(),
            })
        })
        .collect();
    local::json_response(
        StatusCode::OK,
        &ModelList {
            object: "list",
            data,
        },
    )
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

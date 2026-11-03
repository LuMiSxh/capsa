// SPDX-License-Identifier: MPL-2.0
//! Decides which headers cross the proxy and which header values may reach the log.

use reqwest::header::{CONNECTION, CONTENT_LENGTH, HOST, HeaderMap, HeaderName};
use std::collections::BTreeMap;

const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

const SESSION_ID: &str = "x-claude-code-session-id";
const AGENT_ID: &str = "x-claude-code-agent-id";

/// The longest header value the log keeps.
const VALUE_LIMIT: usize = 256;

/// Removes the headers that describe one connection, and those named in `connection`.
pub fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|n| HeaderName::from_bytes(n.trim().as_bytes()).ok())
        .collect();
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
    for name in named {
        headers.remove(name);
    }
}

/// Removes what a forwarded request recomputes: `host` and `content-length`.
pub fn strip_request_framing(headers: &mut HeaderMap) {
    headers.remove(HOST);
    headers.remove(CONTENT_LENGTH);
}

/// What the log keeps of a request's headers.
#[derive(Debug, Default)]
pub struct HeaderLog {
    /// Every header name, never a value.
    pub names: Vec<String>,
    /// The values of the allowlist: `anthropic-beta`, `anthropic-version`, `x-claude-code-*`.
    pub values: BTreeMap<String, String>,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
}

impl HeaderLog {
    /// Reads the loggable parts of `headers`.
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let mut log = HeaderLog::default();
        for (name, value) in headers {
            let name = name.as_str();
            log.names.push(name.to_owned());
            if !allowed(name) {
                continue;
            }
            let Ok(text) = value.to_str() else {
                continue;
            };
            let text: String = text.chars().take(VALUE_LIMIT).collect();
            match name {
                SESSION_ID => log.session_id = Some(text),
                AGENT_ID => log.agent_id = Some(text),
                _ => {
                    log.values.insert(name.to_owned(), text);
                }
            }
        }
        log
    }
}

fn allowed(name: &str) -> bool {
    name == "anthropic-beta" || name == "anthropic-version" || name.starts_with("x-claude-code-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn logs_names_of_secret_headers_but_never_their_values() {
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("authorization", "Bearer SECRET-A"),
            ("x-api-key", "SECRET-B"),
            ("cookie", "s=SECRET-C"),
            ("anthropic-beta", "oauth-2025-04-20"),
            ("x-claude-code-session-id", "sess-1"),
            ("x-claude-code-agent-id", "agent-1"),
            ("x-claude-code-agent-type", "custom"),
            ("x-other", "SECRET-D"),
        ] {
            h.insert(HeaderName::from_static(k), HeaderValue::from_static(v));
        }
        let log = HeaderLog::from_headers(&h);
        let text = format!("{log:?}");
        for secret in ["SECRET-A", "SECRET-B", "SECRET-C", "SECRET-D"] {
            assert!(!text.contains(secret), "{secret} leaked");
        }
        assert!(log.names.contains(&"authorization".to_owned()));
        assert_eq!(log.session_id.as_deref(), Some("sess-1"));
        assert_eq!(log.agent_id.as_deref(), Some("agent-1"));
        assert_eq!(log.values["anthropic-beta"], "oauth-2025-04-20");
        assert_eq!(log.values["x-claude-code-agent-type"], "custom");
    }

    #[test]
    fn strips_hop_by_hop_headers_and_those_connection_names() {
        let mut h = HeaderMap::new();
        h.insert(
            "connection",
            HeaderValue::from_static("keep-alive, x-private"),
        );
        h.insert("x-private", HeaderValue::from_static("1"));
        h.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        h.insert("retry-after", HeaderValue::from_static("3"));
        strip_hop_by_hop(&mut h);
        assert_eq!(h.len(), 1);
        assert!(h.contains_key("retry-after"));
    }
}

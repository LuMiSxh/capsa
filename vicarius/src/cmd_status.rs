// SPDX-License-Identifier: MPL-2.0
//! `vicarius status`: the service's gate, balance, refresh times, requests, tokens and agents.
//! It exits 2 with a hint when the service does not answer.

use std::time::Duration;

use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::{ResultExt, hinted};
use capsa_core::render::{Doc, Render, emit};
use serde::{Serialize, Serializer};
use serde_json::Value;
use vicarius::config::Config;

use crate::status_client::{self, AgentStatus, Fetched, Route, money, relative};

/// How long `status` waits for the service.
const TIMEOUT: Duration = Duration::from_secs(1);

pub fn run(format: Format) -> Result<Outcome> {
    let config = Config::load_from(&Config::path()).hinted()?;
    let fetched = status_client::fetch(config.port, TIMEOUT).ok_or_else(|| {
        hinted(
            format!("vicarius does not answer on 127.0.0.1:{}", config.port),
            "start the service with `vicarius self start`",
        )
    })?;
    emit(&StatusOut(fetched), format)
}

/// The status as `status` shows it. JSON is the service's own bytes; the agent and human views
/// read the same fields.
struct StatusOut(Fetched);

impl Serialize for StatusOut {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        Value::serialize(&self.0.raw, serializer)
    }
}

impl Render for StatusOut {
    fn doc(&self, doc: &mut Doc) {
        let status = &self.0.status;
        doc.title("vicarius status");
        let gate = if status.gate.open {
            "open".to_owned()
        } else {
            let reason = status.gate.reason_text.as_deref().unwrap_or("closed");
            format!("closed: {reason}")
        };
        doc.row("gate").text(gate);
        match status.gate.balance {
            Some(amount) => {
                let text = money(amount, status.gate.currency.as_deref());
                doc.row("balance").text(text);
            }
            None => {
                doc.row("balance").dim("not checked yet");
            }
        }
        if status.gate.reason.as_deref() == Some("no_key") {
            doc.hint(
                "put DEEPSEEK_API_KEY in ~/.config/secrets.env, then run `vicarius self start`",
            );
        }
        self.refresh_rows(doc);
        self.request_rows(doc);
        doc.section("agents");
        match &status.agents {
            None => doc.line("not reported by this service"),
            Some(agents) if agents.entries.is_empty() => doc.line("none configured"),
            Some(agents) => {
                for entry in &agents.entries {
                    doc.row(format!("{}-ds", entry.agent))
                        .text(outcome_text(entry));
                }
            }
        }
    }
}

impl StatusOut {
    fn refresh_rows(&self, doc: &mut Doc) {
        let Some(refresh) = &self.0.status.refresh else {
            return;
        };
        match refresh.last_ok_at {
            Some(at) => doc.row("refreshed").text(relative(at)),
            None => doc.row("refreshed").dim("not yet"),
        };
        if let Some(at) = refresh.next_at {
            doc.row("next refresh").text(relative(at));
        }
        if let Some(error) = &refresh.last_error {
            doc.row("refresh error").text(error.as_str());
        }
    }

    fn request_rows(&self, doc: &mut Doc) {
        let counters = &self.0.status.counters;
        doc.section("requests");
        for (name, route) in [
            ("anthropic", &counters.anthropic),
            ("deepseek", &counters.deepseek),
        ] {
            let text = format!("{} requests, {} refused", route.requests, route.refused);
            doc.row(name).text(text);
        }
        for (name, route) in [
            ("anthropic", &counters.anthropic),
            ("deepseek", &counters.deepseek),
        ] {
            doc.row(format!("{name} tokens")).text(token_text(route));
        }
    }
}

fn token_text(route: &Route) -> String {
    format!(
        "in {}, out {}, cache read {}, cache write {}",
        route.input_tokens,
        route.output_tokens,
        route.cache_read_tokens,
        route.cache_creation_tokens
    )
}

fn outcome_text(entry: &AgentStatus) -> String {
    match entry.outcome_detail() {
        Some(detail) => format!("{}: {detail}", entry.outcome_name()),
        None => entry.outcome_name().to_owned(),
    }
}

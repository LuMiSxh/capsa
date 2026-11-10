// SPDX-License-Identifier: MPL-2.0
//! `census session`: what the current session has done so far, from its transcript and the
//! transcripts of its subagents.

use crate::now_secs;
use crate::out::Out;
use crate::usage_views::{k, money};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::hint::ResultExt;
use capsa_core::render::{Doc, Render, emit};
use census::config::Config;
use census::session::{self, Picked, Summary};
use census::usage::refresh_prices;
use serde::Serialize;

#[derive(clap::Args)]
pub struct Args {
    /// Session id. Defaults to `CLAUDE_CODE_SESSION_ID`, then `CLAUDE_SESSION_ID`, else the most
    /// recent session of this directory.
    #[arg(long, value_name = "ID")]
    session: Option<String>,
}

#[derive(Serialize)]
struct SessionOut<'a> {
    session: &'a str,
    picked: Picked,
    #[serde(flatten)]
    summary: &'a Summary,
}

impl Render for SessionOut<'_> {
    fn doc(&self, doc: &mut Doc) {
        let s = self.summary;
        doc.title("census session");
        doc.row("session")
            .text(format!("{} ({})", self.session, self.picked.label()));
        doc.row("cwd")
            .text(s.cwd.as_deref().unwrap_or("unknown").to_owned());
        doc.row("model")
            .text(s.model.as_deref().unwrap_or("unknown").to_owned());
        doc.row("duration").text(duration(s.duration_s));
        doc.row("requests")
            .text(format!("{} ({} subagents)", s.requests, s.subagents));
        doc.row("subscription").text(money(&s.cost));
        if !s.api.is_empty() {
            let api: Vec<String> = s
                .api
                .iter()
                .map(|(provider, cost)| format!("{provider} {}", money(cost)))
                .collect();
            doc.row("api").text(api.join(", "));
        }
        doc.row("spawns").text(spawns(s));
        match &s.context {
            Some(c) => {
                doc.row("context").bar(c.percent / 100.0).text(format!(
                    "{} of {} ({:.0}%), {} left",
                    k(c.tokens as f64),
                    k(c.threshold.tokens as f64),
                    c.percent,
                    k(c.left as f64)
                ));
            }
            None => {
                doc.row("context").text("none");
            }
        }
        doc.row("compactions").text(s.compactions.to_string());
    }
}

pub fn run(a: &Args, out: &Out) -> Result<Outcome> {
    let config = Config::load();
    refresh_prices(now_secs(), config.prices_refresh_days);
    let resolved = session::resolve_here(a.session.as_deref()).hinted()?;
    let summary = session::summarize(&resolved.transcript, &config)
        .with_context(|| format!("cannot read {}", resolved.transcript.display()))?;
    let body = SessionOut {
        session: &resolved.id,
        picked: resolved.picked,
        summary: &summary,
    };
    emit(&body, out.format)
}

/// `2h 13m`, `4m 05s` or `30s`.
fn duration(seconds: u64) -> String {
    match (seconds / 3600, seconds / 60 % 60, seconds % 60) {
        (0, 0, sec) => format!("{sec}s"),
        (0, min, sec) => format!("{min}m {sec:02}s"),
        (h, min, _) => format!("{h}h {min:02}m"),
    }
}

fn spawns(s: &Summary) -> String {
    if s.spawns.is_empty() {
        return "none".to_owned();
    }
    s.spawns
        .iter()
        .map(|(role, n)| format!("{role} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

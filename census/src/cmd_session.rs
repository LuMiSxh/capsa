// SPDX-License-Identifier: MPL-2.0
//! `census session`: what the current session has done so far, from its transcript and the
//! transcripts of its subagents.

use crate::out::Out;
use crate::usage_views::{k, money};
use anyhow::{Context, Result};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use census::session::{self, Picked, Summary};
use serde::Serialize;

#[derive(clap::Args)]
pub struct Args {
    /// Session id. Defaults to `CLAUDE_SESSION_ID`, else the most recent session of this directory.
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

pub fn run(a: &Args, out: &Out) -> Result<Outcome> {
    let resolved = session::resolve_here(a.session.as_deref()).hinted()?;
    let summary = session::summarize(&resolved.transcript)
        .with_context(|| format!("cannot read {}", resolved.transcript.display()))?;
    if out.format == Format::Json {
        let body = SessionOut {
            session: &resolved.id,
            picked: resolved.picked,
            summary: &summary,
        };
        println!("{}", serde_json::to_string(&body)?);
    } else {
        print_text(out, &resolved.id, resolved.picked, &summary);
    }
    Ok(Outcome::Found)
}

fn print_text(out: &Out, id: &str, picked: Picked, s: &Summary) {
    let field = |label: &str, value: &str| println!("{}", out.field(label, value));
    field("session", &format!("{id} ({})", picked.label()));
    field("cwd", s.cwd.as_deref().unwrap_or("unknown"));
    field("model", s.model.as_deref().unwrap_or("unknown"));
    field("duration", &duration(s.duration_s));
    field(
        "requests",
        &format!("{} ({} subagents)", s.requests, s.subagents),
    );
    field("cost", &money(&s.cost));
    field("spawns", &spawns(s));
    let context = match &s.context {
        Some(c) => format!(
            "{} of {} ({:.0}%), {} left",
            k(c.tokens as f64),
            k(c.threshold.tokens as f64),
            c.percent,
            k(c.left as f64)
        ),
        None => "none".to_owned(),
    };
    field("context", &context);
    field("compactions", &s.compactions.to_string());
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

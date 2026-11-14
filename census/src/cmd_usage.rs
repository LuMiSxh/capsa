// SPDX-License-Identifier: MPL-2.0
//! `census usage`: token, cost and tool-call statistics from agent transcripts.

use crate::now_secs;
use crate::out::Out;
use crate::usage_views::{
    AgentsView, ByView, CompareOut, GuardView, HandbacksView, ToolsView, cost_rows, k, money,
    provider_kind, rate, sizes_text, table,
};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::render::{Doc, Render, emit};
use census::config::Config;
use census::limits::{self, Kind};
use census::usage::{
    Report, UsageOpts, WEIGHTS, compare, guard_log_path, parse_date, read_log, refresh_prices,
    report, write_guard_follow,
};
use serde::Serialize;
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// Only transcripts modified in the last N days.
    #[arg(long)]
    days: Option<u64>,
    /// Only sessions whose working directory contains this text.
    #[arg(long)]
    project: Option<String>,
    /// Only requests billed by this provider: `anthropic` (Claude), `deepseek`, `openai` (Codex).
    #[arg(long, value_name = "PROVIDER", value_parser = census::usage::PROVIDERS)]
    provider: Option<String>,
    /// Rows per table.
    #[arg(long, default_value_t = 10)]
    top: usize,
    /// Claude Code projects dir (default ~/.claude/projects).
    #[arg(long, value_parser = capsa_core::cli::home::<PathBuf>)]
    claude_dir: Option<PathBuf>,
    /// Codex sessions dir (default ~/.codex/sessions).
    #[arg(long, value_parser = capsa_core::cli::home::<PathBuf>)]
    codex_dir: Option<PathBuf>,
    /// Count how agents search, read, edit and build (acus vs grep, sed, python, built-in tools).
    #[arg(long)]
    tools: bool,
    /// With --tools: the most frequent commands of the categories that bypass acus, with their
    /// calls and result characters.
    #[arg(long, requires = "tools")]
    misses: bool,
    /// Count `acus guard` decisions per rule, how often the agent followed each one, and list the
    /// calls run through its escape.
    #[arg(long)]
    guard: bool,
    /// With --guard: the decisions per project and rule, so a rule that keeps firing in one repo
    /// shows.
    #[arg(long, requires = "guard")]
    by_project: bool,
    /// One row per role (main, worker, worker-lite, ...): spawns, models, requests, preamble size,
    /// cost, cache hit rate, and cache rewrites (expired-cache writes, unexpected busts and compactions).
    #[arg(long)]
    agents: bool,
    /// Spawns per ISO week and hand-backs: a senior-worker spawn within 30 min after a worker
    /// spawn of the same session ended (heuristic; rate = hand-backs / worker spawns).
    #[arg(long)]
    handbacks: bool,
    /// Report before and after this UTC date (YYYY-MM-DD), split by request time, with the change.
    #[arg(long, value_name = "YYYY-MM-DD")]
    compare: Option<String>,
    /// Expected cost per bucket of the usage pattern: `weekday`, `hour` of the day, or `cycle`,
    /// the hours since the weekly reset.
    #[arg(long, value_name = "KIND", value_parser = ["weekday", "hour", "cycle"])]
    by: Option<String>,
}

/// `census usage --by`: the expected cost of each bucket of the usage pattern.
fn by(kind: &str, out: &Out) -> Result<Outcome> {
    let kind = match kind {
        "weekday" => Kind::Weekday,
        "hour" => Kind::Hour,
        _ => Kind::Cycle,
    };
    let buckets = limits::usage_by(now_secs(), kind)?;
    emit(&ByView(&buckets), out.format)
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    if a.guard {
        return guard(&a, out);
    }
    let config = Config::load();
    refresh_prices(now_secs(), config.prices_refresh_days);
    let provider = a.provider.as_deref().and_then(|name| {
        census::usage::PROVIDERS
            .iter()
            .copied()
            .find(|p| *p == name)
    });
    if let Some(kind) = &a.by {
        if provider.is_some_and(|p| p != "anthropic") {
            return Ok(Outcome::Empty);
        }
        return by(kind, out);
    }
    let opts = UsageOpts {
        claude_root: a.claude_dir,
        codex_root: a.codex_dir,
        days: a.days,
        project: a.project,
        provider,
    };
    if let Some(date) = &a.compare {
        let Some(cut) = parse_date(date) else {
            anyhow::bail!("--compare takes a UTC date as YYYY-MM-DD, not `{date}`");
        };
        let (before, after) = compare(&opts, cut);
        if before.sessions.is_empty() && after.sessions.is_empty() {
            return Ok(Outcome::Empty);
        }
        let body = CompareOut {
            date: date.as_str(),
            before: &before,
            after: &after,
        };
        return emit(&body, out.format);
    }
    let r = report(&opts);
    if r.sessions.is_empty() {
        return Ok(Outcome::Empty);
    }
    if a.tools {
        return emit(&ToolsView::new(&r, a.misses, a.top), out.format);
    }
    if a.handbacks {
        let view = HandbacksView {
            weeks: &r.handbacks,
            roles: &r.handback_roles,
        };
        return emit(&view, out.format);
    }
    if a.agents {
        return emit(&AgentsView(&r.agents), out.format);
    }
    let body = Summary {
        report: &r,
        top: a.top,
        config: &config,
    };
    emit(&body, out.format)
}

/// The report of `census usage`: totals, cost share and context, then the top models, sessions and
/// tools. Its JSON is the report itself.
#[derive(Serialize)]
#[serde(transparent)]
struct Summary<'a> {
    report: &'a Report,
    #[serde(skip)]
    top: usize,
    #[serde(skip)]
    config: &'a Config,
}

impl Render for Summary<'_> {
    fn doc(&self, doc: &mut Doc) {
        let r = self.report;
        doc.title("census usage");
        totals(doc, r, self.config);
        models(doc, r, self.top, self.config);
        sessions(doc, r, self.top);
        tools(doc, r, self.top);
    }
}

/// `x` as a share of `total`, in percent.
fn pct(x: f64, total: f64) -> String {
    format!("{:.1}%", 100.0 * x / total.max(1.0))
}

/// The totals: requests, cost share and context, then the cost, cache and compaction rows.
fn totals(doc: &mut Doc, r: &Report, config: &Config) {
    doc.row("usage").text(format!(
        "{} sessions, {} requests, {} prompts ({:.1} req/prompt), {} weighted input-token equivalents",
        r.sessions.len(),
        r.total.requests,
        r.prompts,
        r.total.requests as f64 / r.prompts.max(1) as f64,
        k(r.weighted)
    ));
    let mut share: Vec<_> = r
        .total
        .classes()
        .into_iter()
        .zip(WEIGHTS)
        .map(|(n, (name, wt))| (name, n as f64 * wt))
        .collect();
    share.sort_by(|a, b| b.1.total_cmp(&a.1));
    let share: Vec<String> = share
        .iter()
        .map(|(n, x)| format!("{n} {}", pct(*x, r.weighted)))
        .collect();
    doc.row("cost share").text(share.join(", "));
    doc.row("context per request").text(format!(
        "p50 {} p90 {} p99 {}",
        k(r.context[0] as f64),
        k(r.context[1] as f64),
        k(r.context[2] as f64)
    ));
    cost_rows(doc, r, config);
}

/// The top models, with their share of the weighted tokens and their cost.
fn models(doc: &mut Doc, r: &Report, top: usize, config: &Config) {
    doc.section("models");
    let rows = r
        .models
        .iter()
        .take(top)
        .map(|m| {
            vec![
                m.model.clone(),
                m.provider.to_owned(),
                provider_kind(config, m.provider).to_owned(),
                m.tokens.requests.to_string(),
                k(m.tokens.weighted()),
                pct(m.tokens.weighted(), r.weighted),
                money(&m.cost),
            ]
        })
        .collect();
    doc.table(table(
        &[
            "model", "provider", "billing", "requests", "weighted", "share", "cost",
        ],
        "lllrrrr",
        rows,
    ));
    let hidden = r.models.len().saturating_sub(top);
    if hidden > 0 {
        doc.more(hidden, "census usage --top N");
    }
}

/// The top sessions, then a line counting the ones left out.
fn sessions(doc: &mut Doc, r: &Report, top: usize) {
    doc.section("sessions");
    let rows = r
        .sessions
        .iter()
        .take(top)
        .map(|s| {
            vec![
                s.agent.to_string(),
                if s.project.is_empty() {
                    "?".to_owned()
                } else {
                    s.project.clone()
                },
                s.id[..s.id.len().min(8)].to_owned(),
                s.tokens.requests.to_string(),
                k(s.weighted),
                pct(s.weighted, r.weighted),
                k(s.max_context as f64),
                s.compactions.to_string(),
                s.model.clone(),
                money(&s.cost),
                rate(s.tokens.cache_hit_rate()),
                sizes_text(&s.pre_compact),
            ]
        })
        .collect();
    doc.table(table(
        &[
            "agent",
            "project",
            "session",
            "requests",
            "weighted",
            "share",
            "max ctx",
            "compactions",
            "model",
            "cost",
            "cache",
            "pre-compact",
        ],
        "lllrrrrrlrrl",
        rows,
    ));
    let hidden = r.sessions.len().saturating_sub(top);
    if hidden > 0 {
        doc.more(hidden, "census usage --top N");
    }
}

/// The search and read chains, then the top tool categories.
fn tools(doc: &mut Doc, r: &Report, top: usize) {
    doc.section("tools (calls, input chars, result chars)");
    let c = &r.chains;
    doc.line(format!(
        "search/read chains (>=3 in a row): {} chains, {:.0}% of {} tool calls, longest {}",
        c.count,
        100.0 * c.calls_in_chains as f64 / c.tool_calls.max(1) as f64,
        c.tool_calls,
        c.longest
    ));
    let rows = r
        .tools
        .iter()
        .take(top)
        .map(|t| {
            vec![
                t.name.clone(),
                t.calls.to_string(),
                k(t.input_chars as f64),
                k(t.result_chars as f64),
            ]
        })
        .collect();
    doc.table(table(&["tool", "calls", "input", "result"], "lrrr", rows));
    let hidden = r.tools.len().saturating_sub(top);
    if hidden > 0 {
        doc.more(hidden, "census usage --top N");
    }
}

/// `census usage --guard`: the guard's decisions per rule, or per project with `--by-project`, then
/// the latest escaped calls. Without `--days` or `--project` it also writes the follow rates to the
/// census bulletin, which the SessionStart path reads.
fn guard(a: &Args, out: &Out) -> Result<Outcome> {
    let now = now_secs();
    let path = guard_log_path().unwrap_or_default();
    let since = a.days.map_or(0, |d| now.saturating_sub(d * 86_400));
    let entries = read_log(since, a.project.as_deref());
    if a.days.is_none() && a.project.is_none() {
        // A failed write only leaves acus without follow rates; the report still prints.
        write_guard_follow(&entries, now).ok();
    }
    let body = GuardView::new(path.display().to_string(), &entries, a.top, a.by_project);
    let outcome = emit(&body, out.format)?;
    Ok(if entries.is_empty() {
        Outcome::Empty
    } else {
        outcome
    })
}

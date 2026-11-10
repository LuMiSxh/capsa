// SPDX-License-Identifier: MPL-2.0
//! `census usage`: token, cost and tool-call statistics from agent transcripts.

use crate::now_secs;
use crate::out::Out;
use crate::usage_views::{
    AgentsView, ByView, CompareOut, HandbacksView, ToolsView, cost_rows, k, money, provider_kind,
    rate, sizes_text, table,
};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::render::{Doc, Render, emit};
use census::config::Config;
use census::limits::{self, Kind};
use census::usage::{Report, UsageOpts, WEIGHTS, compare, parse_date, refresh_prices, report};
use serde::{Deserialize, Serialize};
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
    /// Count `acus guard` decisions per rule and list the calls run through its escape.
    #[arg(long)]
    guard: bool,
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
        return emit(&HandbacksView(&r.handbacks), out.format);
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
        doc.line(format!(
            "… {hidden} more sessions (raise --top or filter with --project/--days)"
        ));
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
}

/// One line of acus's `guard.jsonl`. Fields keep acus's order, so `--json` prints them as acus
/// wrote them; unknown fields are ignored and old rule names still parse.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct GuardEntry {
    ts: u64,
    session: Option<String>,
    cwd: Option<String>,
    tool: Option<String>,
    rule: String,
    kind: String,
    call: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cmd: Option<String>,
}

/// The time of one guard log line, read without deserializing the rest of it.
#[derive(Deserialize)]
struct Stamp {
    #[serde(default)]
    ts: u64,
}

/// The decision log acus writes: `guard.jsonl` next to its config file, which `ACUS_CONFIG` moves.
fn guard_log_path() -> PathBuf {
    std::env::var("ACUS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| capsa_core::paths::config_dir("acus").join("config.toml"))
        .with_file_name("guard.jsonl")
}

/// The lines of one log file, read on past a line that is not valid UTF-8. The file ends at its
/// last line or at an I/O error.
fn log_lines(file: std::fs::File) -> impl Iterator<Item = String> {
    let mut reader = std::io::BufReader::new(file);
    std::iter::from_fn(move || {
        loop {
            let mut buf = Vec::new();
            match std::io::BufRead::read_until(&mut reader, b'\n', &mut buf) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            if buf.last() == Some(&b'\n') {
                buf.pop();
            }
            if let Ok(line) = String::from_utf8(buf) {
                return Some(line);
            }
        }
    })
}

/// One rule of `census usage --guard` with its decisions, as its JSON.
#[derive(Serialize)]
struct RuleCount<'a> {
    rule: &'a str,
    deny: u64,
    hint: u64,
    escape: u64,
    rewrite: u64,
}

/// `census usage --guard`: per rule how often it refused, hinted, was escaped or rewrote, then the
/// latest escaped calls, since they point at refusals acus could not replace.
#[derive(Serialize)]
struct GuardOut<'a> {
    #[serde(skip)]
    path: String,
    #[serde(skip)]
    decisions: usize,
    rules: Vec<RuleCount<'a>>,
    escaped: Vec<&'a GuardEntry>,
}

impl Render for GuardOut<'_> {
    fn doc(&self, doc: &mut Doc) {
        doc.title(format!(
            "guard: {} decisions ({})",
            self.decisions, self.path
        ));
        let rows = self
            .rules
            .iter()
            .map(|r| {
                vec![
                    r.rule.to_owned(),
                    r.deny.to_string(),
                    r.hint.to_string(),
                    r.escape.to_string(),
                    r.rewrite.to_string(),
                ]
            })
            .collect();
        doc.table(table(
            &["rule", "deny", "hint", "escape", "rewrite"],
            "lrrrr",
            rows,
        ));
        if !self.escaped.is_empty() {
            doc.section("latest escaped calls");
            let rows = self
                .escaped
                .iter()
                .map(|e| vec![e.rule.clone(), e.call.replace('\n', "⏎")])
                .collect();
            doc.table(table(&["rule", "call"], "ll", rows));
        }
    }
}

fn guard(a: &Args, out: &Out) -> Result<Outcome> {
    let path = guard_log_path();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let since = a.days.map_or(0, |d| now.saturating_sub(d * 86_400));
    // Lines are streamed, and each is parsed in full only when its time is in range. A line that
    // is not valid UTF-8 is skipped, and the file is read on past it.
    let entries: Vec<GuardEntry> = [path.with_extension("jsonl.old"), path.clone()]
        .iter()
        .filter_map(|p| std::fs::File::open(p).ok())
        .flat_map(log_lines)
        .filter(|l| serde_json::from_str::<Stamp>(l).is_ok_and(|s| s.ts >= since))
        .filter_map(|l| serde_json::from_str::<GuardEntry>(&l).ok())
        .filter(|e| {
            a.project
                .as_ref()
                .is_none_or(|p| e.cwd.as_deref().unwrap_or_default().contains(p.as_str()))
        })
        .collect();
    if entries.is_empty() {
        eprintln!(
            "no guard decisions in {}\nhint: add the acus guard PreToolUse hook (acus self setup)",
            path.display()
        );
        return Ok(Outcome::Empty);
    }
    let mut counts: std::collections::BTreeMap<&str, [u64; 4]> = Default::default();
    for e in &entries {
        let slot = match e.kind.as_str() {
            "deny" => 0,
            "hint" => 1,
            "rewrite" => 3,
            _ => 2,
        };
        counts.entry(e.rule.as_str()).or_default()[slot] += 1;
    }
    let escaped: Vec<&GuardEntry> = entries
        .iter()
        .filter(|e| e.kind == "escape")
        .rev()
        .take(a.top)
        .collect();
    let body = GuardOut {
        path: path.display().to_string(),
        decisions: entries.len(),
        rules: counts
            .iter()
            .map(|(r, n)| RuleCount {
                rule: r,
                deny: n[0],
                hint: n[1],
                escape: n[2],
                rewrite: n[3],
            })
            .collect(),
        escaped,
    };
    emit(&body, out.format)
}

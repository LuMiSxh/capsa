// SPDX-License-Identifier: MPL-2.0
//! `census usage`: token, cost and tool-call statistics from agent transcripts.

use crate::out::Out;
use crate::usage_views::{compare_out, handbacks, k, money, pre_list, print_cost_cache, rate, usd};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use census::usage::{
    AgentStat, CategoryStat, Report, UsageOpts, WEIGHTS, compare, group, parse_date, report,
};
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
    /// Rows per table.
    #[arg(long, default_value_t = 10)]
    top: usize,
    /// Claude Code projects dir (default ~/.claude/projects).
    #[arg(long, value_parser = crate::home::<PathBuf>)]
    claude_dir: Option<PathBuf>,
    /// Codex sessions dir (default ~/.codex/sessions).
    #[arg(long, value_parser = crate::home::<PathBuf>)]
    codex_dir: Option<PathBuf>,
    /// Count how agents search, read, edit and build (acus vs grep, sed, python, built-in tools).
    #[arg(long)]
    tools: bool,
    /// Count `acus guard` decisions per rule and list the calls run through its escape.
    #[arg(long)]
    guard: bool,
    /// One row per role (main, worker, worker-lite, ...): spawns, models, requests, preamble size,
    /// cost, cache hit rate, and cache rewrites (expired-cache writes and unexpected busts).
    #[arg(long)]
    agents: bool,
    /// Spawns per ISO week and hand-backs: a senior-worker spawn within 30 min after a worker
    /// spawn of the same session ended (heuristic; rate = hand-backs / worker spawns).
    #[arg(long)]
    handbacks: bool,
    /// Report before and after this UTC date (YYYY-MM-DD), split by request time, with the change.
    #[arg(long, value_name = "YYYY-MM-DD")]
    compare: Option<String>,
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    if a.guard {
        return guard(&a, out);
    }
    let opts = UsageOpts {
        claude_root: a.claude_dir,
        codex_root: a.codex_dir,
        days: a.days,
        project: a.project,
    };
    if let Some(date) = &a.compare {
        let Some(cut) = parse_date(date) else {
            anyhow::bail!("--compare takes a UTC date as YYYY-MM-DD, not `{date}`");
        };
        let (before, after) = compare(&opts, cut);
        return compare_out(date, &before, &after, out);
    }
    let r = report(&opts);
    if r.sessions.is_empty() {
        return Ok(Outcome::Empty);
    }
    if a.tools {
        return tools(&r, out);
    }
    if a.handbacks {
        return handbacks(&r.handbacks, out);
    }
    if a.agents {
        return agents(&r.agents, out);
    }
    if out.format == Format::Json {
        println!("{}", serde_json::to_string(&r)?);
        return Ok(Outcome::Found);
    }
    print_summary(&r, out, a.top);
    Ok(Outcome::Found)
}

/// The human-readable report: totals, cost share, context, then the top models, sessions and tools.
fn print_summary(r: &Report, out: &Out, top: usize) {
    let w = r.weighted.max(1.0);
    let pct = |x: f64| format!("{:.1}%", 100.0 * x / w);
    println!(
        "usage: {} sessions, {} requests, {} prompts ({:.1} req/prompt), {} weighted input-token equivalents",
        r.sessions.len(),
        r.total.requests,
        r.prompts,
        r.total.requests as f64 / r.prompts.max(1) as f64,
        k(r.weighted)
    );
    let mut share: Vec<_> = r
        .total
        .classes()
        .into_iter()
        .zip(WEIGHTS)
        .map(|(n, (name, wt))| (name, n as f64 * wt))
        .collect();
    share.sort_by(|a, b| b.1.total_cmp(&a.1));
    let share: Vec<_> = share
        .iter()
        .map(|(n, x)| format!("{n} {}", pct(*x)))
        .collect();
    println!("cost share: {}", share.join(", "));
    println!(
        "context per request: p50 {} p90 {} p99 {}",
        k(r.context[0] as f64),
        k(r.context[1] as f64),
        k(r.context[2] as f64)
    );
    print_cost_cache(r);
    println!("{}", out.header("models"));
    for ((m, t), (_, c)) in r.models.iter().zip(&r.model_costs).take(top) {
        println!(
            "  {m} reqs={} w={} ({}) cost={}",
            t.requests,
            k(t.weighted()),
            pct(t.weighted()),
            money(c)
        );
    }
    println!("{}", out.header("sessions"));
    for s in r.sessions.iter().take(top) {
        println!(
            "  {} {} {} reqs={} w={} ({}) maxctx={} compact={} {} cost={} cache={}{}",
            s.agent,
            if s.project.is_empty() {
                "?"
            } else {
                &s.project
            },
            &s.id[..s.id.len().min(8)],
            s.tokens.requests,
            k(s.weighted),
            pct(s.weighted),
            k(s.max_context as f64),
            s.compactions,
            s.model,
            money(&s.cost),
            rate(s.tokens.cache_hit_rate()),
            pre_list(&s.pre_compact)
        );
    }
    println!("{}", out.header("tools (calls, input chars, result chars)"));
    for t in r.tools.iter().take(top) {
        println!(
            "  {} {} in={} out={}",
            t.name,
            t.calls,
            k(t.input_chars as f64),
            k(t.result_chars as f64)
        );
    }
    let c = &r.chains;
    println!(
        "search/read chains (>=3 in a row): {} chains, {:.0}% of {} tool calls, longest {}",
        c.count,
        100.0 * c.calls_in_chains as f64 / c.tool_calls.max(1) as f64,
        c.tool_calls,
        c.longest
    );
    let hidden = r.sessions.len().saturating_sub(top);
    if hidden > 0 {
        println!("… {hidden} more sessions (raise --top or filter with --project/--days)");
    }
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

/// The decision log acus writes: `guard.jsonl` next to its config file, which `ACUS_CONFIG` moves.
fn guard_log_path() -> PathBuf {
    std::env::var("ACUS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| capsa_core::paths::config_dir("acus").join("config.toml"))
        .with_file_name("guard.jsonl")
}

/// Reads the guard log: per rule how often it refused, hinted or was escaped. Escaped calls
/// are listed, since they point at refusals acus could not replace.
fn guard(a: &Args, out: &Out) -> Result<Outcome> {
    let path = guard_log_path();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let since = a.days.map_or(0, |d| now.saturating_sub(d * 86_400));
    let entries: Vec<GuardEntry> = [path.with_extension("jsonl.old"), path.clone()]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .flat_map(|s| {
            s.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect::<Vec<_>>()
        })
        .filter(|e: &GuardEntry| e.ts >= since)
        .filter(|e| {
            a.project
                .as_ref()
                .is_none_or(|p| e.cwd.as_deref().unwrap_or_default().contains(p.as_str()))
        })
        .collect();
    if entries.is_empty() {
        eprintln!(
            "no guard decisions in {}\nhint: add the acus guard PreToolUse hook (acus self skill --install --hooks)",
            path.display()
        );
        return Ok(Outcome::Empty);
    }
    let mut rules: std::collections::BTreeMap<&str, [u64; 4]> = Default::default();
    for e in &entries {
        let slot = match e.kind.as_str() {
            "deny" => 0,
            "hint" => 1,
            "rewrite" => 3,
            _ => 2,
        };
        rules.entry(e.rule.as_str()).or_default()[slot] += 1;
    }
    let escaped: Vec<&GuardEntry> = entries
        .iter()
        .filter(|e| e.kind == "escape")
        .rev()
        .take(a.top)
        .collect();
    if out.format == Format::Json {
        let v: Vec<_> = rules
            .iter()
            .map(
                |(r, n)| serde_json::json!({"rule": r, "deny": n[0], "hint": n[1], "escape": n[2], "rewrite": n[3]}),
            )
            .collect();
        println!("{}", serde_json::json!({"rules": v, "escaped": escaped}));
        return Ok(Outcome::Found);
    }
    println!("guard: {} decisions ({})", entries.len(), path.display());
    println!("{}", out.header("rule deny hint escape rewrite"));
    for (r, n) in &rules {
        println!("  {r} {} {} {} {}", n[0], n[1], n[2], n[3]);
    }
    if !escaped.is_empty() {
        println!("{}", out.header("latest escaped calls"));
        for e in escaped {
            println!("  {} {}", e.rule, e.call.replace('\n', "⏎"));
        }
    }
    Ok(Outcome::Found)
}

/// Codex transcripts carry no roles and count under `main`.
fn agents(rows: &[AgentStat], out: &Out) -> Result<Outcome> {
    if out.format == Format::Json {
        println!("{}", serde_json::to_string(rows)?);
        return Ok(Outcome::Found);
    }
    println!(
        "{}",
        out.header(
            "role spawns models median-reqs median-first-ctx weighted share median-weighted/spawn cost cache-hit expired expired-cost busts"
        )
    );
    for a in rows {
        println!(
            "  {} {} {} {:.0} {} {} {:.1}% {} {} {} {} {} {}",
            a.role,
            a.spawns,
            a.models.join(","),
            a.median_requests,
            k(a.median_first_context),
            k(a.weighted),
            100.0 * a.share,
            k(a.median_weighted),
            money(&a.cost),
            rate(a.tokens.cache_hit_rate()),
            a.cache.expired,
            usd(a.cache.expired_usd),
            a.cache.busts
        );
    }
    Ok(Outcome::Found)
}

fn tools(r: &Report, out: &Out) -> Result<Outcome> {
    let mut rows: Vec<_> = r
        .categories
        .iter()
        .map(|(c, s)| (group(c), *c, *s))
        .collect();
    rows.sort_by_key(|&(g, c, s)| (g, std::cmp::Reverse(total_calls(&s)), c));
    let group_sum = |g: &str, f: fn(&CategoryStat) -> u64| {
        rows.iter()
            .filter(|row| row.0 == g)
            .map(|row| f(&row.2))
            .sum::<u64>()
            .max(1)
    };
    if out.format == Format::Json {
        let v: Vec<_> = rows
            .iter()
            .map(|&(g, c, s)| {
                serde_json::json!({
                    "group": g, "category": c, "claude": s.calls[0], "codex": s.calls[1],
                    "total": total_calls(&s),
                    "share": total_calls(&s) as f64 / group_sum(g, total_calls) as f64,
                    "result_chars": total_chars(&s),
                    "result_share": total_chars(&s) as f64 / group_sum(g, total_chars) as f64,
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&v)?);
        return Ok(Outcome::Found);
    }
    println!(
        "tool usage: {} sessions; a call counts once per category it hits",
        r.sessions.len()
    );
    println!(
        "{}",
        out.header("category claude codex total share-of-group result-chars result-share-of-group")
    );
    for &(g, c, s) in &rows {
        println!(
            "  {g}/{c} {} {} {} {:.0}% {} {:.0}%",
            s.calls[0],
            s.calls[1],
            total_calls(&s),
            100.0 * total_calls(&s) as f64 / group_sum(g, total_calls) as f64,
            k(total_chars(&s) as f64),
            100.0 * total_chars(&s) as f64 / group_sum(g, total_chars) as f64,
        );
    }
    Ok(Outcome::Found)
}

fn total_calls(s: &CategoryStat) -> u64 {
    s.calls.iter().sum()
}

fn total_chars(s: &CategoryStat) -> u64 {
    s.result_chars.iter().sum()
}

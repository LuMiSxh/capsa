// SPDX-License-Identifier: MPL-2.0
//! Owns the printed views of `census usage` that the summary and the table commands share.

use crate::out::Out;
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use census::limits::{Kind, Pattern, Table};
use census::usage::{AgentStat, Cost, HandbackWeek, Report};

/// Prints the buckets of `census usage --by`: expected cost, share and hours behind each.
pub(crate) fn by_table(t: &Table, out: &Out) -> Result<()> {
    if out.format == Format::Json {
        println!("{}", serde_json::to_string(t)?);
        return Ok(());
    }
    if !t.has_data() {
        println!("no usage data yet: no transcript to read");
        return Ok(());
    }
    let kind = match t.kind {
        Kind::Weekday => "weekday",
        Kind::Hour => "hour of the day",
        Kind::Cycle => "hour since the weekly reset",
    };
    println!("{}", out.header(&format!("expected cost per {kind}")));
    if t.kind == Kind::Cycle && t.basis.pattern != Pattern::SinceReset {
        println!("needs two weeks of weekly snapshots to map the reset; zero until then");
    }
    println!("{:<10}{:>10}{:>9}{:>8}", "bucket", "usd", "share", "hours");
    for b in &t.rows {
        println!(
            "{:<10}{:>10}{:>9}{:>8}",
            b.label,
            usd(b.usd),
            format!("{:.1} %", 100.0 * b.share),
            b.hours
        );
    }
    let basis = format!(
        "{:.1} weeks of UTC history; pattern {}",
        t.basis.weeks,
        t.basis.pattern.label()
    );
    println!("{}", out.field("basis", &basis));
    Ok(())
}

/// 1234567 → "1.2M".
pub(crate) fn k(n: f64) -> String {
    match n {
        n if n >= 1e9 => format!("{:.1}G", n / 1e9),
        n if n >= 1e6 => format!("{:.1}M", n / 1e6),
        n if n >= 1e3 => format!("{:.0}k", n / 1e3),
        n => format!("{n:.0}"),
    }
}

/// `$1.23`.
pub(crate) fn usd(x: f64) -> String {
    format!("${x:.2}")
}

/// `$1.23`, `n/a` when no request has a price, and `(+N unpriced)` beside a partial price.
pub(crate) fn money(c: &Cost) -> String {
    match c.unpriced {
        0 => usd(c.usd),
        _ if c.usd <= 0.0 => "n/a".into(),
        n => format!("{} (+{n} unpriced)", usd(c.usd)),
    }
}

/// `93.1%`, or `n/a` without a value.
pub(crate) fn rate(x: Option<f64>) -> String {
    x.map_or_else(|| "n/a".into(), |x| format!("{:.1}%", 100.0 * x))
}

/// ` pre=180k,224k` for the context sizes before a session's compactions, or nothing.
pub(crate) fn pre_list(sizes: &[u64]) -> String {
    if sizes.is_empty() {
        return String::new();
    }
    let sizes: Vec<String> = sizes.iter().map(|&n| k(n as f64)).collect();
    format!(" pre={}", sizes.join(","))
}

/// Hand-backs per worker spawn over all weeks, or `None` without worker spawns.
fn hand_back_rate(rows: &[HandbackWeek]) -> Option<f64> {
    let workers: u64 = rows.iter().map(|w| w.workers).sum();
    let handbacks: u64 = rows.iter().map(|w| w.handbacks).sum();
    (workers > 0).then(|| handbacks as f64 / workers as f64)
}

/// Spawns and hand-backs per ISO week. The hand-back count is a heuristic, and every format says so.
pub(crate) fn handbacks(rows: &[HandbackWeek], out: &Out) -> Result<Outcome> {
    if out.format == Format::Json {
        println!("{}", serde_json::to_string(rows)?);
        return Ok(Outcome::Found);
    }
    println!(
        "hand-back heuristic: a senior-worker spawn starting within 30 min after a worker spawn of the same session ended (or started) counts; rate = hand-backs / worker spawns"
    );
    println!(
        "{}",
        out.header("week workers senior-workers hand-backs rate")
    );
    for w in rows {
        println!(
            "  {} {} {} {} {}",
            w.week,
            w.workers,
            w.seniors,
            w.handbacks,
            rate(w.rate)
        );
    }
    let total = |f: fn(&HandbackWeek) -> u64| rows.iter().map(f).sum::<u64>();
    println!(
        "  total {} {} {} {}",
        total(|w| w.workers),
        total(|w| w.seniors),
        total(|w| w.handbacks),
        rate(hand_back_rate(rows))
    );
    Ok(Outcome::Found)
}

/// The cost, cache and compaction lines of the summary.
pub(crate) fn print_cost_cache(r: &Report) {
    println!(
        "cost: {} at list prices, unpriced requests: {}",
        usd(r.cost.usd),
        r.cost.unpriced
    );
    println!(
        "cache: hit {}, expired-cache rewrites {} ({}), unexpected busts {}",
        rate(r.total.cache_hit_rate()),
        r.cache.expired,
        usd(r.cache.expired_usd),
        r.cache.busts
    );
    println!(
        "compactions: {} in {} sessions, context before them p50 {} p90 {}",
        r.compactions,
        r.sessions.iter().filter(|s| s.compactions > 0).count(),
        k(r.compaction_context[0] as f64),
        k(r.compaction_context[1] as f64)
    );
}

/// `--compare`: the totals, then each role, before and after the date with the change.
pub(crate) fn compare_out(date: &str, b: &Report, a: &Report, out: &Out) -> Result<Outcome> {
    if b.sessions.is_empty() && a.sessions.is_empty() {
        return Ok(Outcome::Empty);
    }
    if out.format == Format::Json {
        println!(
            "{}",
            serde_json::json!({ "date": date, "before": b, "after": a })
        );
        return Ok(Outcome::Found);
    }
    let int = |n: f64| format!("{n:.0}");
    println!("compare at {date} (UTC): before -> after, change");
    println!("{}", out.header("totals"));
    num_row(
        "sessions",
        b.sessions.len() as f64,
        a.sessions.len() as f64,
        int,
    );
    num_row(
        "requests",
        b.total.requests as f64,
        a.total.requests as f64,
        int,
    );
    num_row("weighted", b.weighted, a.weighted, k);
    num_row("cost", b.cost.usd, a.cost.usd, usd);
    rate_row(
        "cache hit",
        b.total.cache_hit_rate(),
        a.total.cache_hit_rate(),
    );
    rate_row(
        "hand-back rate",
        hand_back_rate(&b.handbacks),
        hand_back_rate(&a.handbacks),
    );
    num_row(
        "expired",
        b.cache.expired as f64,
        a.cache.expired as f64,
        int,
    );
    num_row(
        "expired cost",
        b.cache.expired_usd,
        a.cache.expired_usd,
        usd,
    );
    num_row("busts", b.cache.busts as f64, a.cache.busts as f64, int);
    num_row(
        "compactions",
        b.compactions as f64,
        a.compactions as f64,
        int,
    );
    println!(
        "{}",
        out.header("roles: spawns, weighted per spawn, cache hit, expired rewrites")
    );
    let mut names: Vec<&str> = b
        .agents
        .iter()
        .chain(&a.agents)
        .map(|s| s.role.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let (x, y) = (role_of(b, name), role_of(a, name));
        println!("  {name}");
        let spawns = |s: Option<&AgentStat>| s.map_or(0.0, |s| s.spawns as f64);
        num_row("spawns", spawns(x), spawns(y), int);
        let per = |s: Option<&AgentStat>| {
            s.filter(|s| s.spawns > 0)
                .map_or(0.0, |s| s.weighted / s.spawns as f64)
        };
        num_row("weighted/spawn", per(x), per(y), k);
        let hit = |s: Option<&AgentStat>| s.and_then(|s| s.tokens.cache_hit_rate());
        rate_row("cache hit", hit(x), hit(y));
        let expired = |s: Option<&AgentStat>| s.map_or(0.0, |s| s.cache.expired as f64);
        num_row("expired", expired(x), expired(y), int);
    }
    Ok(Outcome::Found)
}

/// The row of `role` in a report, if it had spawns.
fn role_of<'a>(r: &'a Report, role: &str) -> Option<&'a AgentStat> {
    r.agents.iter().find(|s| s.role == role)
}

/// One compare row: both values and the change in percent.
fn num_row(name: &str, before: f64, after: f64, show: fn(f64) -> String) {
    let change = if before > 0.0 {
        format!("{:+.0}%", 100.0 * (after - before) / before)
    } else {
        "n/a".into()
    };
    println!(
        "  {name:<16} {:>10} -> {:>10}  {change}",
        show(before),
        show(after)
    );
}

/// One compare row of rates: both values and the change in percentage points.
fn rate_row(name: &str, before: Option<f64>, after: Option<f64>) {
    let change = match (before, after) {
        (Some(b), Some(a)) => format!("{:+.1}pp", 100.0 * (a - b)),
        _ => "n/a".into(),
    };
    println!(
        "  {name:<16} {:>10} -> {:>10}  {change}",
        rate(before),
        rate(after)
    );
}

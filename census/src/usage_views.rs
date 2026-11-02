// SPDX-License-Identifier: MPL-2.0
//! The views of `census usage` that are tables: buckets, hand-backs, roles, compare and tools.
//! It also holds the formatting helpers that the report commands share.

use capsa_core::exit::Outcome;
use capsa_core::render::{Align, Doc, Render, Table};
use census::limits::{Kind, Pattern, Table as Buckets};
use census::usage::{AgentStat, CategoryStat, Cost, HandbackWeek, Report, group};
use serde::Serialize;

/// A table with one header per column; `align` holds an `r` for each right-aligned column.
pub(crate) fn table(head: &[&str], align: &str, rows: Vec<Vec<String>>) -> Table {
    Table {
        head: head.iter().map(|h| (*h).to_owned()).collect(),
        align: align
            .chars()
            .map(|c| if c == 'r' { Align::Right } else { Align::Left })
            .collect(),
        rows,
    }
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

/// `$1.23`. Adding zero turns the `-0.0` of an empty sum into `0.0`, which would print as `$-0.00`.
pub(crate) fn usd(x: f64) -> String {
    format!("${:.2}", x + 0.0)
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

/// The context sizes before a session's compactions, as `180k,224k`; empty without compactions.
pub(crate) fn sizes_text(sizes: &[u64]) -> String {
    sizes
        .iter()
        .map(|&n| k(n as f64))
        .collect::<Vec<_>>()
        .join(",")
}

/// The cost, cache and compaction rows of the summary.
pub(crate) fn cost_rows(doc: &mut Doc, r: &Report) {
    doc.row("cost").text(format!(
        "{} at list prices, unpriced requests: {}",
        usd(r.cost.usd),
        r.cost.unpriced
    ));
    doc.row("cache").text(format!(
        "hit {}, expired-cache rewrites {} ({}), unexpected busts {}",
        rate(r.total.cache_hit_rate()),
        r.cache.expired,
        usd(r.cache.expired_usd),
        r.cache.busts
    ));
    doc.row("compactions").text(format!(
        "{} in {} sessions, context before them p50 {} p90 {}",
        r.compactions,
        r.sessions.iter().filter(|s| s.compactions > 0).count(),
        k(r.compaction_context[0] as f64),
        k(r.compaction_context[1] as f64)
    ));
}

/// Hand-backs per worker spawn over all weeks, or `None` without worker spawns.
fn hand_back_rate(rows: &[HandbackWeek]) -> Option<f64> {
    let workers: u64 = rows.iter().map(|w| w.workers).sum();
    let handbacks: u64 = rows.iter().map(|w| w.handbacks).sum();
    (workers > 0).then(|| handbacks as f64 / workers as f64)
}

/// The row of `census usage --by`: the expected cost of each bucket of the usage pattern, as its JSON.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct ByView<'a>(pub &'a Buckets);

impl Render for ByView<'_> {
    fn doc(&self, doc: &mut Doc) {
        let t = self.0;
        doc.title("census usage --by");
        if !t.has_data() {
            doc.line("no usage data yet: no transcript to read");
            return;
        }
        let kind = match t.kind {
            Kind::Weekday => "weekday",
            Kind::Hour => "hour of the day",
            Kind::Cycle => "hour since the weekly reset",
        };
        doc.row("basis").text(format!(
            "{:.1} weeks of local history; pattern {}",
            t.basis.weeks,
            t.basis.pattern.label()
        ));
        if t.kind == Kind::Cycle && t.basis.pattern != Pattern::SinceReset {
            doc.line("needs two weeks of weekly snapshots to map the reset; zero until then");
        }
        doc.section(format!("expected cost per {kind}"));
        let rows = t
            .rows
            .iter()
            .map(|b| {
                vec![
                    b.label.clone(),
                    usd(b.usd),
                    format!("{:.1} %", 100.0 * b.share),
                    b.hours.to_string(),
                ]
            })
            .collect();
        doc.table(table(&["bucket", "usd", "share", "hours"], "lrrr", rows));
    }

    fn outcome(&self) -> Outcome {
        if self.0.has_data() {
            Outcome::Found
        } else {
            Outcome::Empty
        }
    }
}

/// `census usage --handbacks`: spawns and hand-backs per ISO week, as its JSON. The hand-back count
/// is a heuristic, and the view says so.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct HandbacksView<'a>(pub &'a [HandbackWeek]);

impl Render for HandbacksView<'_> {
    fn doc(&self, doc: &mut Doc) {
        let rows = self.0;
        doc.title("census usage --handbacks");
        doc.line(
            "hand-back heuristic: a senior-worker spawn starting within 30 min after a worker spawn of the same session ended (or started) counts; rate = hand-backs / worker spawns",
        );
        let total = |f: fn(&HandbackWeek) -> u64| rows.iter().map(f).sum::<u64>();
        let mut body: Vec<Vec<String>> = rows
            .iter()
            .map(|w| {
                vec![
                    w.week.clone(),
                    w.workers.to_string(),
                    w.seniors.to_string(),
                    w.handbacks.to_string(),
                    rate(w.rate),
                ]
            })
            .collect();
        body.push(vec![
            "total".to_owned(),
            total(|w| w.workers).to_string(),
            total(|w| w.seniors).to_string(),
            total(|w| w.handbacks).to_string(),
            rate(hand_back_rate(rows)),
        ]);
        doc.table(table(
            &["week", "workers", "senior-workers", "hand-backs", "rate"],
            "lrrrr",
            body,
        ));
    }
}

/// `census usage --agents`: one row per role, as its JSON. Codex transcripts carry no roles and
/// count under `main`.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct AgentsView<'a>(pub &'a [AgentStat]);

impl Render for AgentsView<'_> {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --agents");
        let rows = self
            .0
            .iter()
            .map(|a| {
                vec![
                    a.role.clone(),
                    a.spawns.to_string(),
                    a.models.join(","),
                    format!("{:.0}", a.median_requests),
                    k(a.median_first_context),
                    k(a.weighted),
                    format!("{:.1}%", 100.0 * a.share),
                    k(a.median_weighted),
                    money(&a.cost),
                    rate(a.tokens.cache_hit_rate()),
                    a.cache.expired.to_string(),
                    usd(a.cache.expired_usd),
                    a.cache.busts.to_string(),
                    a.cache.compactions.to_string(),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "role",
                "spawns",
                "models",
                "median-reqs",
                "median-first-ctx",
                "weighted",
                "share",
                "median-weighted/spawn",
                "cost",
                "cache-hit",
                "expired",
                "expired-cost",
                "busts",
                "compactions",
            ],
            "lrlrrrrrrrrrrr",
            rows,
        ));
    }
}

/// `census usage --compare`: the totals and each role, before and after a date, as its JSON.
#[derive(Serialize)]
pub(crate) struct CompareOut<'a> {
    pub date: &'a str,
    pub before: &'a Report,
    pub after: &'a Report,
}

impl Render for CompareOut<'_> {
    fn doc(&self, doc: &mut Doc) {
        let (b, a) = (self.before, self.after);
        let int = |n: f64| format!("{n:.0}");
        let totals = vec![
            labelled(
                "sessions",
                num_cells(b.sessions.len() as f64, a.sessions.len() as f64, int),
            ),
            labelled(
                "requests",
                num_cells(b.total.requests as f64, a.total.requests as f64, int),
            ),
            labelled("weighted", num_cells(b.weighted, a.weighted, k)),
            labelled("cost", num_cells(b.cost.usd, a.cost.usd, usd)),
            labelled(
                "cache hit",
                rate_cells(b.total.cache_hit_rate(), a.total.cache_hit_rate()),
            ),
            labelled(
                "hand-back rate",
                rate_cells(hand_back_rate(&b.handbacks), hand_back_rate(&a.handbacks)),
            ),
            labelled(
                "expired",
                num_cells(b.cache.expired as f64, a.cache.expired as f64, int),
            ),
            labelled(
                "expired cost",
                num_cells(b.cache.expired_usd, a.cache.expired_usd, usd),
            ),
            labelled(
                "busts",
                num_cells(b.cache.busts as f64, a.cache.busts as f64, int),
            ),
            labelled(
                "compactions",
                num_cells(b.compactions as f64, a.compactions as f64, int),
            ),
        ];
        let mut names: Vec<&str> = b
            .agents
            .iter()
            .chain(&a.agents)
            .map(|s| s.role.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        let mut roles: Vec<Vec<String>> = Vec::new();
        for name in names {
            let (x, y) = (role_of(b, name), role_of(a, name));
            let spawns = |s: Option<&AgentStat>| s.map_or(0.0, |s| s.spawns as f64);
            let per = |s: Option<&AgentStat>| {
                s.filter(|s| s.spawns > 0)
                    .map_or(0.0, |s| s.weighted / s.spawns as f64)
            };
            let hit = |s: Option<&AgentStat>| s.and_then(|s| s.tokens.cache_hit_rate());
            let expired = |s: Option<&AgentStat>| s.map_or(0.0, |s| s.cache.expired as f64);
            let lines = [
                ("spawns", num_cells(spawns(x), spawns(y), int)),
                ("weighted/spawn", num_cells(per(x), per(y), k)),
                ("cache hit", rate_cells(hit(x), hit(y))),
                ("expired", num_cells(expired(x), expired(y), int)),
            ];
            for (i, (metric, cells)) in lines.into_iter().enumerate() {
                let role = if i == 0 { name } else { "" };
                roles.push(
                    std::iter::once(role.to_owned())
                        .chain(std::iter::once(metric.to_owned()))
                        .chain(cells)
                        .collect(),
                );
            }
        }
        doc.title(format!("compare at {} (UTC)", self.date));
        doc.section("totals");
        doc.table(table(
            &["metric", "before", "after", "change"],
            "lrrr",
            totals,
        ));
        doc.section("roles: spawns, weighted per spawn, cache hit, expired rewrites");
        doc.table(table(
            &["role", "metric", "before", "after", "change"],
            "llrrr",
            roles,
        ));
    }
}

/// The row of `role` in a report, if it had spawns.
fn role_of<'a>(r: &'a Report, role: &str) -> Option<&'a AgentStat> {
    r.agents.iter().find(|s| s.role == role)
}

/// A table row: its name, then its cells.
fn labelled(name: &str, cells: Vec<String>) -> Vec<String> {
    std::iter::once(name.to_owned()).chain(cells).collect()
}

/// Before, after and the change in percent; `n/a` without a base.
fn num_cells(before: f64, after: f64, show: fn(f64) -> String) -> Vec<String> {
    let change = if before > 0.0 {
        format!("{:+.0}%", 100.0 * (after - before) / before)
    } else {
        "n/a".into()
    };
    vec![show(before), show(after), change]
}

/// Before, after and the change in percentage points.
fn rate_cells(before: Option<f64>, after: Option<f64>) -> Vec<String> {
    let change = match (before, after) {
        (Some(b), Some(a)) => format!("{:+.1}pp", 100.0 * (a - b)),
        _ => "n/a".into(),
    };
    vec![rate(before), rate(after), change]
}

/// `census usage --tools`: one row per tool category, with its share of the group, as its JSON.
#[derive(Serialize)]
struct ToolRow {
    group: String,
    category: String,
    claude: u64,
    codex: u64,
    total: u64,
    share: f64,
    result_chars: u64,
    result_share: f64,
}

#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct ToolsView {
    #[serde(skip)]
    sessions: usize,
    rows: Vec<ToolRow>,
}

impl ToolsView {
    pub(crate) fn new(r: &Report) -> Self {
        ToolsView {
            sessions: r.sessions.len(),
            rows: tool_rows(r),
        }
    }
}

impl Render for ToolsView {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --tools");
        doc.line(format!(
            "tool usage: {} sessions; a call counts once per category it hits",
            self.sessions
        ));
        let rows = self
            .rows
            .iter()
            .map(|t| {
                vec![
                    format!("{}/{}", t.group, t.category),
                    t.claude.to_string(),
                    t.codex.to_string(),
                    t.total.to_string(),
                    format!("{:.0}%", 100.0 * t.share),
                    k(t.result_chars as f64),
                    format!("{:.0}%", 100.0 * t.result_share),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "category",
                "claude",
                "codex",
                "total",
                "share-of-group",
                "result-chars",
                "result-share-of-group",
            ],
            "lrrrrrr",
            rows,
        ));
    }
}

/// The tool categories, sorted by group and then by calls, with the shares of their group.
fn tool_rows(r: &Report) -> Vec<ToolRow> {
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
    rows.iter()
        .map(|&(g, c, s)| ToolRow {
            group: g.to_owned(),
            category: c.to_owned(),
            claude: s.calls[0],
            codex: s.calls[1],
            total: total_calls(&s),
            share: total_calls(&s) as f64 / group_sum(g, total_calls) as f64,
            result_chars: total_chars(&s),
            result_share: total_chars(&s) as f64 / group_sum(g, total_chars) as f64,
        })
        .collect()
}

fn total_calls(s: &CategoryStat) -> u64 {
    s.calls.iter().sum()
}

fn total_chars(s: &CategoryStat) -> u64 {
    s.result_chars.iter().sum()
}

// SPDX-License-Identifier: MPL-2.0
//! Folds the transcripts of a report into sessions, models, roles, tool statistics and hand-backs.

use super::output::{CONTEXT_BUCKETS, GAP_BUCKETS, REWRITE_GAP_MS, REWRITE_TOKENS, bucket_of};
use super::prices::write_cost;
use super::returns::{self, ReturnAcc};
use super::skills::{self, SkillAcc};
use super::{
    ContextBucket, Cost, FileStats, GapBucket, MissStat, OutputStat, Report, Request, Session,
    SpawnCall, Tokens, ToolStat, handback,
};
use agents::{Spawn, agent_stats};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

mod agents;

/// Cache TTL of a main thread and of a subagent thread, in milliseconds.
const MAIN_TTL: u64 = 60 * 60 * 1000;
const SUBAGENT_TTL: u64 = 5 * 60 * 1000;

/// Which requests a report counts: all of them, or those before or from a time (ms since the epoch).
#[derive(Clone, Copy)]
pub(super) enum Side {
    All,
    Before(u64),
    After(u64),
}

impl Side {
    /// Requests without a timestamp count as before any cut.
    fn keeps(self, ts: u64) -> bool {
        match self {
            Side::All => true,
            Side::Before(cut) => ts < cut,
            Side::After(cut) => ts >= cut,
        }
    }
}

/// Folds `files` into one report over the requests `side` keeps. Cache gaps are judged on every
/// request of a thread, so a cut does not hide the request that follows it.
pub(super) fn build(
    files: &[FileStats],
    side: Side,
    prefixes: &BTreeMap<String, String>,
) -> Report {
    let mut acc = Acc::default();
    for f in files {
        acc.file(f, side);
    }
    acc.finish(prefixes)
}

/// What a report folds its transcripts into.
#[derive(Default)]
struct Acc<'a> {
    r: Report,
    seen: HashSet<&'a str>,
    sessions: HashMap<(&'static str, String), Session>,
    models: HashMap<&'a str, (Tokens, Cost)>,
    tools: HashMap<String, ToolStat>,
    contexts: Vec<u64>,
    context_buckets: HashMap<(&'static str, usize), ContextAcc>,
    compactions: Vec<u64>,
    roles: HashMap<String, Vec<Spawn>>,
    spawns: Vec<(&'a str, &'a SpawnCall)>,
    misses: HashMap<(&'static str, String), MissAcc>,
    gaps: [GapAcc; 4],
    returns: HashMap<String, ReturnAcc>,
    skills: HashMap<(String, &'static str), SkillAcc>,
}

impl<'a> Acc<'a> {
    fn file(&mut self, f: &'a FileStats, side: Side) {
        let mut s = self.take_session(f);
        add_calls(&mut self.r, &mut self.tools, &mut self.misses, f);
        skills::add_file(&mut self.skills, f);
        returns::add_file(&mut self.returns, f);
        let mut spawn = self.fold_requests(f, side, &mut s);
        spawn.tool_chars += f.calls.iter().map(|c| c.input_chars).sum::<u64>();
        self.fold_compactions(f, side, &mut s);
        if spawn.requests > 0 {
            self.roles.entry(f.role.clone()).or_default().push(spawn);
        }
        for c in f.spawns.iter().filter(|c| side.keeps(c.start_ms)) {
            self.spawns.push((f.session.as_str(), c));
            returns::add_spawn(&mut self.returns, c);
        }
        self.sessions.insert((f.agent, f.session.clone()), s);
    }

    /// Takes the session of `f` out of the map, or starts one, so that its requests can be folded
    /// while the rest of the report is borrowed.
    fn take_session(&mut self, f: &FileStats) -> Session {
        let project = Path::new(&f.cwd)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut s = self
            .sessions
            .remove(&(f.agent, f.session.clone()))
            .unwrap_or_else(|| Session {
                agent: f.agent,
                project: String::new(),
                id: f.session.clone(),
                model: String::new(),
                tokens: Tokens::default(),
                weighted: 0.0,
                max_context: 0,
                compactions: 0,
                pre_compact: Vec::new(),
                cost: Cost::default(),
                prompts: 0,
            });
        if s.project.is_empty() {
            s.project = project;
        }
        s
    }

    /// Folds the requests of `f` that `side` keeps into `s` and the report, and returns the spawn
    /// they add up to. A request whose id was already seen is skipped.
    fn fold_requests(&mut self, f: &'a FileStats, side: Side, s: &mut Session) -> Spawn {
        let ttl = if f.is_subagent {
            SUBAGENT_TTL
        } else {
            MAIN_TTL
        };
        let mut spawn = Spawn::default();
        let mut prev = None;
        let mut prev_context = 0;
        for q in &f.requests {
            let gap = prev
                .filter(|&p| p > 0 && q.ts > 0)
                .map(|p| q.ts.saturating_sub(p));
            prev = Some(q.ts);
            let keeps = side.keeps(q.ts);
            if keeps {
                spawn.text_chars += q.text_chars;
            }
            if q.id.as_deref().is_some_and(|id| !self.seen.insert(id)) {
                continue;
            }
            // A context under half of the previous request's is a compaction, as is a request after
            // a compact boundary; its cache writes are neither expired nor a bust.
            let compacted = q.after_compact || 2 * q.tokens.context() < prev_context;
            prev_context = q.tokens.context();
            if !keeps {
                continue;
            }
            let t = &q.tokens;
            let mut cost = Cost::default();
            cost.add_request(&q.model, t);
            if let Some(g) = gap {
                self.gap(g, q);
            }
            self.context(f.is_subagent, q, &cost);
            match gap {
                _ if compacted && t.cache_writes() > 0 => spawn.cache.compactions += 1,
                Some(g) if g > ttl && t.cache_writes() > 0 => {
                    spawn.cache.expired += 1;
                    spawn.cache.expired_usd += write_cost(&q.model, t).unwrap_or(0.0);
                }
                Some(g) if g < ttl && 2 * t.cache_writes() > t.context() => {
                    spawn.cache.busts += 1;
                }
                _ => {}
            }
            spawn.add(&q.model, t, &cost, q.thinking);
            s.tokens.add(t);
            s.cost.merge(&cost);
            s.max_context = s.max_context.max(t.context());
            self.contexts.push(t.context());
            let m = self.models.entry(q.model.as_str()).or_default();
            m.0.add(t);
            m.1.merge(&cost);
            self.r.total.add(t);
            self.r.cost.merge(&cost);
            if s.model.is_empty() || s.model == "<synthetic>" {
                s.model.clone_from(&q.model);
            }
        }
        spawn
    }

    /// Folds the compactions of `f` that `side` keeps, and its prompts when `side` is all requests.
    fn fold_compactions(&mut self, f: &FileStats, side: Side, s: &mut Session) {
        for c in &f.compactions {
            if side.keeps(c.ts) {
                s.compactions += 1;
                s.pre_compact.push(c.pre_tokens);
                self.compactions.push(c.pre_tokens);
            }
        }
        if matches!(side, Side::All) {
            s.prompts += f.prompts;
            self.r.prompts += f.prompts;
        }
    }

    /// Adds one request to the pause bucket of `gap` milliseconds and to the context bucket of its
    /// context size. A cache write after a pause over five minutes is a rewrite when it exceeds 5k
    /// tokens.
    fn gap(&mut self, gap: u64, q: &Request) {
        let i = bucket_of(&GAP_BUCKETS, gap);
        let b = &mut self.gaps[i];
        b.requests += 1;
        b.cache_read += q.tokens.cache_read;
        b.cache_writes += q.tokens.cache_writes();
        b.output += q.tokens.output;
        if gap > REWRITE_GAP_MS && q.tokens.cache_writes() > REWRITE_TOKENS {
            b.rewrites += 1;
            b.rewrite_usd += write_cost(&q.model, &q.tokens).unwrap_or(0.0);
        }
    }

    fn context(&mut self, subagent: bool, q: &Request, cost: &Cost) {
        let role = if subagent { "subagent" } else { "main" };
        let i = bucket_of(&CONTEXT_BUCKETS, q.tokens.context());
        let b = self.context_buckets.entry((role, i)).or_default();
        b.requests += 1;
        b.cost.merge(cost);
        b.cache_read += q.tokens.cache_read;
        b.cache_writes += q.tokens.cache_writes();
        b.output += q.tokens.output;
    }

    fn finish(self, prefixes: &BTreeMap<String, String>) -> Report {
        let mut r = self.r;
        r.weighted = r.total.weighted();
        r.context = quantiles(self.contexts);
        r.compactions = self.compactions.len() as u64;
        r.compaction_context = quantiles(self.compactions);
        r.sessions = self
            .sessions
            .into_values()
            .filter(|s| s.tokens.requests > 0 || s.prompts > 0)
            .collect();
        for s in &mut r.sessions {
            s.weighted = s.tokens.weighted();
        }
        r.sessions
            .sort_by(|a, b| b.weighted.total_cmp(&a.weighted).then(a.id.cmp(&b.id)));
        let mut providers: BTreeMap<String, (Tokens, Cost)> = BTreeMap::new();
        let mut models: Vec<super::ModelStat> = self
            .models
            .into_iter()
            .map(|(m, (tokens, cost))| {
                let provider = super::prices::billed_by(m, prefixes).to_owned();
                let p = providers.entry(provider.clone()).or_default();
                p.0.add(&tokens);
                p.1.merge(&cost);
                super::ModelStat {
                    provider,
                    model: m.to_owned(),
                    tokens,
                    cost,
                }
            })
            .collect();
        models.sort_by(|a, b| b.tokens.weighted().total_cmp(&a.tokens.weighted()));
        r.models = models;
        r.providers = providers
            .into_iter()
            .map(|(provider, (tokens, cost))| super::ProviderStat {
                provider,
                tokens,
                cost,
            })
            .collect();
        r.tools = self.tools.into_values().collect();
        r.result_chars = r.tools.iter().map(|t| t.result_chars).sum();
        r.tools
            .sort_by(|a, b| b.calls.cmp(&a.calls).then(a.name.cmp(&b.name)));
        for s in r.categories.values_mut() {
            s.sizes.sort_unstable();
        }
        let mut misses: Vec<MissStat> = self
            .misses
            .into_iter()
            .filter(|(_, m)| m.calls > 0)
            .map(|((category, command), m)| MissStat {
                category,
                command,
                calls: m.calls,
                intercepted: m.intercepted,
                result_chars: m.result_chars,
            })
            .collect();
        misses.sort_by(|a, b| {
            b.calls
                .cmp(&a.calls)
                .then(a.category.cmp(b.category))
                .then(a.command.cmp(&b.command))
        });
        r.misses = misses;
        for spawn in self.roles.values().flatten() {
            r.cache.add(&spawn.cache);
        }
        r.agents = agent_stats(self.roles, r.weighted);
        r.handbacks = handback::weeks(&self.spawns);
        r.handback_roles = handback::roles(&self.spawns);
        r.output = output_total(&r.agents);
        r.gaps = gap_buckets(self.gaps);
        r.contexts = context_buckets(self.context_buckets);
        r.returns = returns::finish(self.returns, &r.agents);
        r.skills = skills::finish(self.skills);
        r
    }
}

/// The requests of one pause bucket.
#[derive(Default, Clone, Copy)]
struct GapAcc {
    requests: u64,
    cache_read: u64,
    cache_writes: u64,
    output: u64,
    rewrites: u64,
    rewrite_usd: f64,
}

/// The requests of one role group at one context size.
#[derive(Default)]
struct ContextAcc {
    requests: u64,
    cost: Cost,
    cache_read: u64,
    cache_writes: u64,
    output: u64,
}

/// Returns the output split of the whole report from its role rows.
fn output_total(agents: &[super::AgentStat]) -> OutputStat {
    let mut o = OutputStat::default();
    for a in agents {
        o.requests += a.tokens.requests;
        o.output += a.tokens.output;
        o.thinking += a.thinking;
        o.text_chars += a.text_chars;
        o.tool_chars += a.tool_chars;
    }
    o
}

/// Returns the gap buckets in pause order.
fn gap_buckets(acc: [GapAcc; 4]) -> Vec<GapBucket> {
    acc.into_iter()
        .zip(GAP_BUCKETS)
        .map(|(a, (_, label))| GapBucket {
            label,
            requests: a.requests,
            cache_read: a.cache_read,
            cache_writes: a.cache_writes,
            output: a.output,
            rewrites: a.rewrites,
            rewrite_usd: a.rewrite_usd,
        })
        .collect()
}

/// Returns the context buckets, main before subagent and the smallest context first.
fn context_buckets(acc: HashMap<(&'static str, usize), ContextAcc>) -> Vec<ContextBucket> {
    let mut rows: Vec<ContextBucket> = acc
        .into_iter()
        .map(|((role, i), a)| ContextBucket {
            role,
            label: CONTEXT_BUCKETS[i].1,
            requests: a.requests,
            cost: a.cost,
            cache_read: a.cache_read,
            cache_writes: a.cache_writes,
            output: a.output,
        })
        .collect();
    rows.sort_by_key(|r| (r.role != "main", r.role, context_index(r.label)));
    rows
}

fn context_index(label: &str) -> usize {
    CONTEXT_BUCKETS
        .iter()
        .position(|(_, l)| *l == label)
        .unwrap_or(0)
}

/// The 50th, 90th and 99th percentile of `values`, or 0 for each when there are none.
fn quantiles(mut values: Vec<u64>) -> [u64; 3] {
    values.sort_unstable();
    [0.5, 0.9, 0.99].map(|q| super::percentile(&values, q))
}

/// Adds one transcript's tool calls to the tool, category and chain statistics.
///
/// A miss counts the calls of a shape that ran unguarded; a call the guard denied or rewrote is
/// tallied as intercepted and adds neither a call nor result characters.
fn add_calls(
    r: &mut Report,
    tools: &mut HashMap<String, ToolStat>,
    misses: &mut HashMap<(&'static str, String), MissAcc>,
    f: &FileStats,
) {
    let mut run = 0u64;
    let agent = usize::from(f.agent == "codex");
    for c in &f.calls {
        let t = tools.entry(c.name.clone()).or_insert_with(|| ToolStat {
            name: c.name.clone(),
            ..Default::default()
        });
        t.calls += 1;
        t.input_chars += c.input_chars;
        t.result_chars += c.result_chars;
        r.chains.tool_calls += 1;
        for &cat in &c.categories {
            let s = r.categories.entry(cat).or_default();
            s.calls[agent] += 1;
            s.result_chars[agent] += c.result_chars;
            s.sizes.push(c.result_chars);
            // Every category outside `acus:*` is a call the guard never sees.
            if !cat.starts_with("acus:")
                && let Some((_, command)) = c.commands.iter().find(|(c, _)| *c == cat)
            {
                let m = misses.entry((cat, command.clone())).or_default();
                if c.intercepted {
                    m.intercepted += 1;
                } else {
                    m.calls += 1;
                    m.result_chars += c.result_chars;
                }
            }
        }
        if c.explore {
            run += 1;
        } else {
            close_chain(&mut r.chains, &mut run);
        }
    }
    close_chain(&mut r.chains, &mut run);
    if f.orphan_result_chars > 0 {
        tools
            .entry("?".into())
            .or_insert_with(|| ToolStat {
                name: "?".into(),
                ..Default::default()
            })
            .result_chars += f.orphan_result_chars;
    }
}

/// The calls of one command shape that bypasses acus.
#[derive(Default)]
struct MissAcc {
    /// Calls that ran unguarded.
    calls: u64,
    /// Calls the guard denied or rewrote.
    intercepted: u64,
    result_chars: u64,
}

fn close_chain(c: &mut super::Chains, run: &mut u64) {
    if *run >= 3 {
        c.count += 1;
        c.calls_in_chains += *run;
        c.longest = c.longest.max(*run);
    }
    *run = 0;
}

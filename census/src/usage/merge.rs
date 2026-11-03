// SPDX-License-Identifier: MPL-2.0
//! Folds the transcripts of a report into sessions, models, roles, tool statistics and hand-backs.

use super::prices::write_cost;
use super::{
    AgentStat, CacheStat, Cost, FileStats, Report, Session, SpawnCall, Tokens, ToolStat, handback,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

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
pub(super) fn build(files: &[FileStats], side: Side) -> Report {
    let mut acc = Acc::default();
    for f in files {
        acc.file(f, side);
    }
    acc.finish()
}

/// What a report folds its transcripts into.
#[derive(Default)]
struct Acc<'a> {
    r: Report,
    seen: HashSet<&'a str>,
    sessions: HashMap<(&'static str, String), Session>,
    models: HashMap<&'a str, (Tokens, Cost)>,
    providers: HashMap<&'static str, (Tokens, Cost)>,
    tools: HashMap<String, ToolStat>,
    contexts: Vec<u64>,
    compactions: Vec<u64>,
    roles: HashMap<String, Vec<Spawn>>,
    spawns: Vec<(&'a str, &'a SpawnCall)>,
}

impl<'a> Acc<'a> {
    fn file(&mut self, f: &'a FileStats, side: Side) {
        let project = Path::new(&f.cwd)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let s = self
            .sessions
            .entry((f.agent, f.session.clone()))
            .or_insert_with(|| Session {
                agent: f.agent,
                project: project.clone(),
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
        add_calls(&mut self.r, &mut self.tools, f);
        let ttl = if f.is_subagent {
            SUBAGENT_TTL
        } else {
            MAIN_TTL
        };
        let mut spawn = Spawn::default();
        let mut prev = None;
        let mut prev_context = 0;
        for q in &f.requests {
            if q.id.as_deref().is_some_and(|id| !self.seen.insert(id)) {
                continue;
            }
            let gap = prev
                .filter(|&p| p > 0 && q.ts > 0)
                .map(|p| q.ts.saturating_sub(p));
            prev = Some(q.ts);
            // A context under half of the previous request's is a compaction, as is a request after
            // a compact boundary; its cache writes are neither expired nor a bust.
            let compacted = q.after_compact || 2 * q.tokens.context() < prev_context;
            prev_context = q.tokens.context();
            if !side.keeps(q.ts) {
                continue;
            }
            let t = &q.tokens;
            let mut cost = Cost::default();
            cost.add_request(&q.model, t);
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
            spawn.add(&q.model, t, &cost);
            s.tokens.add(t);
            s.cost.merge(&cost);
            s.max_context = s.max_context.max(t.context());
            self.contexts.push(t.context());
            let m = self.models.entry(q.model.as_str()).or_default();
            m.0.add(t);
            m.1.merge(&cost);
            let p = self
                .providers
                .entry(super::prices::billed_by(&q.model))
                .or_default();
            p.0.add(t);
            p.1.merge(&cost);
            self.r.total.add(t);
            self.r.cost.merge(&cost);
            if s.model.is_empty() || s.model == "<synthetic>" {
                s.model.clone_from(&q.model);
            }
        }
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
        if spawn.requests > 0 {
            self.roles.entry(f.role.clone()).or_default().push(spawn);
        }
        self.spawns.extend(
            f.spawns
                .iter()
                .filter(|c| side.keeps(c.start_ms))
                .map(|c| (f.session.as_str(), c)),
        );
    }

    fn finish(self) -> Report {
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
        let mut models: Vec<super::ModelStat> = self
            .models
            .into_iter()
            .map(|(m, (tokens, cost))| super::ModelStat {
                provider: super::prices::billed_by(m),
                model: m.to_owned(),
                tokens,
                cost,
            })
            .collect();
        models.sort_by(|a, b| b.tokens.weighted().total_cmp(&a.tokens.weighted()));
        r.models = models;
        let mut providers: Vec<super::ProviderStat> = self
            .providers
            .into_iter()
            .map(|(provider, (tokens, cost))| super::ProviderStat {
                provider,
                tokens,
                cost,
            })
            .collect();
        providers.sort_by_key(|p| p.provider);
        r.providers = providers;
        r.tools = self.tools.into_values().collect();
        r.result_chars = r.tools.iter().map(|t| t.result_chars).sum();
        r.tools
            .sort_by(|a, b| b.calls.cmp(&a.calls).then(a.name.cmp(&b.name)));
        for spawn in self.roles.values().flatten() {
            r.cache.add(&spawn.cache);
        }
        r.agents = agent_stats(self.roles, r.weighted);
        r.handbacks = handback::weeks(&self.spawns);
        r
    }
}

/// One transcript's requests after dedup.
#[derive(Default)]
struct Spawn {
    requests: u64,
    first_context: u64,
    weighted: f64,
    models: HashMap<String, u64>,
    tokens: Tokens,
    cost: Cost,
    cache: CacheStat,
}

impl Spawn {
    fn add(&mut self, model: &str, t: &Tokens, cost: &Cost) {
        if self.requests == 0 {
            self.first_context = t.context();
        }
        self.requests += 1;
        self.weighted += t.weighted();
        self.tokens.add(t);
        self.cost.merge(cost);
        if !model.is_empty() && model != "<synthetic>" {
            *self.models.entry(model.to_owned()).or_default() += 1;
        }
    }
}

/// The 50th, 90th and 99th percentile of `values`, or 0 for each when there are none.
fn quantiles(mut values: Vec<u64>) -> [u64; 3] {
    values.sort_unstable();
    [0.5, 0.9, 0.99].map(|q| {
        values
            .get((values.len() as f64 * q) as usize)
            .copied()
            .unwrap_or(0)
    })
}

fn agent_stats(roles: HashMap<String, Vec<Spawn>>, total: f64) -> Vec<AgentStat> {
    let mut out: Vec<AgentStat> = roles
        .into_iter()
        .map(|(role, spawns)| {
            let col = |f: fn(&Spawn) -> f64| median(spawns.iter().map(f).collect());
            let mut models: HashMap<&str, u64> = HashMap::new();
            for (m, n) in spawns.iter().flat_map(|s| &s.models) {
                *models.entry(m).or_default() += n;
            }
            let mut models: Vec<_> = models.into_iter().collect();
            models.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let weighted = spawns.iter().map(|s| s.weighted).sum::<f64>();
            let mut tokens = Tokens::default();
            let mut cost = Cost::default();
            let mut cache = CacheStat::default();
            for s in &spawns {
                tokens.add(&s.tokens);
                cost.merge(&s.cost);
                cache.add(&s.cache);
            }
            AgentStat {
                spawns: spawns.len() as u64,
                models: models.into_iter().map(|(m, _)| m.to_owned()).collect(),
                median_requests: col(|s| s.requests as f64),
                median_first_context: col(|s| s.first_context as f64),
                weighted,
                share: weighted / total.max(1.0),
                median_weighted: col(|s| s.weighted),
                tokens,
                cost,
                cache,
                role,
            }
        })
        .collect();
    out.sort_by(|a, b| b.weighted.total_cmp(&a.weighted).then(a.role.cmp(&b.role)));
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

/// Adds one transcript's tool calls to the tool, category and chain statistics.
fn add_calls(r: &mut Report, tools: &mut HashMap<String, ToolStat>, f: &FileStats) {
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

fn close_chain(c: &mut super::Chains, run: &mut u64) {
    if *run >= 3 {
        c.count += 1;
        c.calls_in_chains += *run;
        c.longest = c.longest.max(*run);
    }
    *run = 0;
}

// SPDX-License-Identifier: MPL-2.0
//! Token and tool-call analytics over Claude Code (`~/.claude/projects`) and Codex
//! (`~/.codex/sessions`) JSONL transcripts.

mod claude;
mod codex;
mod read;
mod tools;
mod walk;

pub use tools::group;

use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Default, Clone)]
pub struct UsageOpts {
    /// Defaults to `$CLAUDE_CONFIG_DIR/projects` or `~/.claude/projects`.
    pub claude_root: Option<PathBuf>,
    /// Defaults to `$CODEX_HOME/sessions` or `~/.codex/sessions`.
    pub codex_root: Option<PathBuf>,
    /// Only transcripts modified within this many days.
    pub days: Option<u64>,
    /// Case-insensitive substring of the session's working directory.
    pub project: Option<String>,
}

/// Relative cost in input-token equivalents (Anthropic list-price ratios):
/// cache read 0.1x, 5-minute cache write 1.25x, 1-hour cache write 2x, output 5x.
pub const WEIGHTS: [(&str, f64); 5] = [
    ("input", 1.0),
    ("cache_read", 0.1),
    ("cache_write_5m", 1.25),
    ("cache_write_1h", 2.0),
    ("output", 5.0),
];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    pub requests: u64,
}

impl Tokens {
    pub fn classes(&self) -> [u64; 5] {
        [
            self.input,
            self.cache_read,
            self.cache_write_5m,
            self.cache_write_1h,
            self.output,
        ]
    }

    pub fn weighted(&self) -> f64 {
        self.classes()
            .iter()
            .zip(WEIGHTS)
            .map(|(n, (_, w))| *n as f64 * w)
            .sum()
    }

    /// Tokens the model had to look at for this request.
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write_5m + self.cache_write_1h
    }

    fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.cache_read += o.cache_read;
        self.cache_write_5m += o.cache_write_5m;
        self.cache_write_1h += o.cache_write_1h;
        self.output += o.output;
        self.requests += o.requests;
    }
}

#[derive(Debug, Serialize)]
pub struct Session {
    pub agent: &'static str,
    pub project: String,
    pub id: String,
    pub model: String,
    pub tokens: Tokens,
    pub weighted: f64,
    pub max_context: u64,
    pub compactions: u32,
    pub prompts: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct ToolStat {
    pub name: String,
    pub calls: u64,
    pub input_chars: u64,
    pub result_chars: u64,
}

/// Runs of ≥3 consecutive search/read tool calls.
#[derive(Debug, Default, Serialize)]
pub struct Chains {
    pub count: u64,
    pub calls_in_chains: u64,
    pub tool_calls: u64,
    pub longest: u64,
}

/// One row of `census usage --agents`: every transcript is one spawn of a role
/// (`main` for top-level sessions, the subagent type otherwise).
#[derive(Debug, Default, Serialize)]
pub struct AgentStat {
    pub role: String,
    pub spawns: u64,
    pub models: Vec<String>,
    pub median_requests: f64,
    /// Median of input + cache read + cache write of each spawn's first request.
    pub median_first_context: f64,
    pub weighted: f64,
    pub share: f64,
    pub median_weighted: f64,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub total: Tokens,
    pub weighted: f64,
    pub prompts: u64,
    /// Requests' context size percentiles: p50, p90, p99.
    pub context: [u64; 3],
    pub models: Vec<(String, Tokens)>,
    pub sessions: Vec<Session>,
    pub tools: Vec<ToolStat>,
    pub chains: Chains,
    pub agents: Vec<AgentStat>,
    /// Tool-call categories → calls and result characters per agent `[claude, codex]`.
    #[serde(skip)]
    pub categories: BTreeMap<&'static str, CategoryStat>,
    /// Characters of all tool results.
    pub result_chars: u64,
}

/// Calls and tool-result characters of one category, per agent `[claude, codex]`.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct CategoryStat {
    pub calls: [u64; 2],
    pub result_chars: [u64; 2],
}

/// What one transcript file contributes.
#[derive(Default)]
pub(crate) struct FileStats {
    pub agent: &'static str,
    pub session: String,
    pub cwd: String,
    /// Subagent type (`main` for top-level sessions); empty until resolved.
    pub role: String,
    pub is_subagent: bool,
    /// Parent tool call that spawned this subagent (from the meta file).
    pub spawn_id: Option<String>,
    /// (tool_use id, subagent_type) of Agent calls made in this file.
    pub spawns: Vec<(String, String)>,
    /// (dedup id, model, tokens of one request)
    pub requests: Vec<(Option<String>, String, Tokens)>,
    pub compactions: u32,
    pub prompts: u64,
    pub calls: Vec<Call>,
    /// Result characters of results whose call is not in this file.
    pub orphan_result_chars: u64,
}

/// One tool call of a transcript, with the characters of the result that answers it.
pub(crate) struct Call {
    pub name: String,
    pub input_chars: u64,
    /// Whether the call only searches or reads, see `is_explore_cmd`.
    pub explore: bool,
    /// Categories the call hits, see `tools::classify`.
    pub categories: Vec<&'static str>,
    pub result_chars: u64,
}

/// Shell commands whose first real step only searches or reads (`cd x && rg …` counts).
pub(crate) fn is_explore_cmd(cmd: &str) -> bool {
    for part in cmd.split(['&', ';', '|', '\n']) {
        let first = part.split_whitespace().next().unwrap_or("");
        match first.rsplit('/').next().unwrap_or(first) {
            "" | "cd" | "pwd" | "echo" | "printf" | "set" => continue,
            "grep" | "rg" | "sed" | "cat" | "head" | "tail" | "find" | "fd" | "ls" | "wc"
            | "acus" | "tree" | "nl" => return true,
            _ => return false,
        }
    }
    false
}

/// Characters of a tool result's text: a JSON string, or the `text` blocks of a JSON array.
pub(crate) fn text_chars(raw: &str) -> u64 {
    #[derive(serde::Deserialize)]
    struct Text {
        text: Option<String>,
    }
    let chars = |s: &str| s.chars().count() as u64;
    if raw.starts_with('"') {
        return serde_json::from_str::<String>(raw).map_or(0, |s| chars(&s));
    }
    serde_json::from_str::<Vec<Text>>(raw).map_or(0, |v| {
        v.iter()
            .filter_map(|t| t.text.as_deref())
            .map(|s| s.chars().count() as u64)
            .sum()
    })
}

fn default_root(env: &str, home_sub: &str, sub: &str) -> Option<PathBuf> {
    std::env::var_os(env)
        .map(|d| PathBuf::from(d).join(sub))
        .or_else(|| std::env::home_dir().map(|h| h.join(home_sub).join(sub)))
}

pub fn report(o: &UsageOpts) -> Report {
    let claude = o
        .claude_root
        .clone()
        .or_else(|| default_root("CLAUDE_CONFIG_DIR", ".claude", "projects"));
    let codex = o
        .codex_root
        .clone()
        .or_else(|| default_root("CODEX_HOME", ".codex", "sessions"));
    let cutoff = o
        .days
        .map(|d| SystemTime::now() - Duration::from_secs(d * 86_400));
    let mut jobs = Vec::new();
    for (root, agent) in [(claude, "claude"), (codex, "codex")] {
        let Some(root) = root.filter(|r| r.is_dir()) else {
            continue;
        };
        jobs.extend(walk::jsonl_files(&root).into_iter().map(|path| read::Job {
            agent,
            root: root.clone(),
            path,
        }));
    }
    let mut files = read::read_all(&jobs, cutoff);
    if let Some(q) = &o.project {
        let q = q.to_lowercase();
        files.retain(|f| f.cwd.to_lowercase().contains(&q));
    }
    // A session takes its project and model from its first file, so the main transcript goes first.
    files.sort_by(|a, b| {
        a.session
            .cmp(&b.session)
            .then(a.is_subagent.cmp(&b.is_subagent))
    });
    // Subagents without an `agentType` take the type of the parent's Agent call.
    let spawned: HashMap<String, String> = files
        .iter()
        .flat_map(|f| f.spawns.iter().cloned())
        .collect();
    for f in files.iter_mut().filter(|f| f.role.is_empty()) {
        f.role = f
            .spawn_id
            .as_ref()
            .and_then(|id| spawned.get(id))
            .cloned()
            .unwrap_or_else(|| "subagent".into());
    }
    merge(files)
}

/// Claude: `<project>/<session>.jsonl` and `<project>/<session>/subagents/*.jsonl`
/// share one session; Codex: one rollout file per session.
fn session_id(root: &Path, p: &Path, agent: &str) -> String {
    let rel = p.strip_prefix(root).unwrap_or(p);
    let part = if agent == "claude" {
        rel.iter().nth(1)
    } else {
        rel.file_name()
    };
    let s = part.map(|s| s.to_string_lossy()).unwrap_or_default();
    let s = s.trim_end_matches(".jsonl");
    // `rollout-<timestamp>-<uuid>`: keep the uuid.
    s.get(s.len().saturating_sub(36)..).unwrap_or(s).to_owned()
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

fn merge(files: Vec<FileStats>) -> Report {
    let mut r = Report::default();
    let mut seen = HashSet::new();
    let mut sessions: HashMap<(&'static str, String), Session> = HashMap::new();
    let mut models: HashMap<String, Tokens> = HashMap::new();
    let mut tools: HashMap<String, ToolStat> = HashMap::new();
    let mut contexts = Vec::new();
    let mut roles: HashMap<String, Vec<Spawn>> = HashMap::new();
    for f in files {
        let project = Path::new(&f.cwd)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let s = sessions
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
                prompts: 0,
            });
        if s.project.is_empty() {
            s.project = project;
        }
        add_calls(&mut r, &mut tools, &f);
        let mut spawn = Spawn::default();
        for (id, model, t) in f.requests {
            if id.is_some_and(|id| !seen.insert(id)) {
                continue;
            }
            spawn.add(&model, &t);
            s.tokens.add(&t);
            s.max_context = s.max_context.max(t.context());
            contexts.push(t.context());
            models.entry(model.clone()).or_default().add(&t);
            r.total.add(&t);
            if s.model.is_empty() || s.model == "<synthetic>" {
                s.model = model;
            }
        }
        s.compactions += f.compactions;
        if spawn.requests > 0 {
            roles.entry(f.role.clone()).or_default().push(spawn);
        }
        s.prompts += f.prompts;
        r.prompts += f.prompts;
    }
    r.weighted = r.total.weighted();
    r.context = quantiles(contexts);
    r.sessions = sessions
        .into_values()
        .filter(|s| s.tokens.requests > 0 || s.prompts > 0)
        .collect();
    for s in &mut r.sessions {
        s.weighted = s.tokens.weighted();
    }
    r.sessions
        .sort_by(|a, b| b.weighted.total_cmp(&a.weighted).then(a.id.cmp(&b.id)));
    r.models = models.into_iter().collect();
    r.models
        .sort_by(|a, b| b.1.weighted().total_cmp(&a.1.weighted()));
    r.tools = tools.into_values().collect();
    r.result_chars = r.tools.iter().map(|t| t.result_chars).sum();
    r.agents = agent_stats(roles, r.weighted);
    r.tools
        .sort_by(|a, b| b.calls.cmp(&a.calls).then(a.name.cmp(&b.name)));
    r
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

/// One transcript's requests after dedup.
#[derive(Default)]
struct Spawn {
    requests: u64,
    first_context: u64,
    weighted: f64,
    models: HashMap<String, u64>,
}

impl Spawn {
    fn add(&mut self, model: &str, t: &Tokens) {
        if self.requests == 0 {
            self.first_context = t.context();
        }
        self.requests += 1;
        self.weighted += t.weighted();
        if !model.is_empty() && model != "<synthetic>" {
            *self.models.entry(model.to_owned()).or_default() += 1;
        }
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
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
            AgentStat {
                spawns: spawns.len() as u64,
                models: models.into_iter().map(|(m, _)| m.to_owned()).collect(),
                median_requests: col(|s| s.requests as f64),
                median_first_context: col(|s| s.first_context as f64),
                weighted,
                share: weighted / total.max(1.0),
                median_weighted: col(|s| s.weighted),
                role,
            }
        })
        .collect();
    out.sort_by(|a, b| b.weighted.total_cmp(&a.weighted).then(a.role.cmp(&b.role)));
    out
}

fn close_chain(c: &mut Chains, run: &mut u64) {
    if *run >= 3 {
        c.count += 1;
        c.calls_in_chains += *run;
        c.longest = c.longest.max(*run);
    }
    *run = 0;
}

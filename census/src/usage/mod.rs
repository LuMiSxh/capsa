// SPDX-License-Identifier: MPL-2.0
//! Token and tool-call analytics over Claude Code (`~/.claude/projects`) and Codex
//! (`~/.codex/sessions`) JSONL transcripts.

mod claude;
mod codex;
mod handback;
mod merge;
mod prices;
mod read;
mod time;
mod tools;
mod walk;

pub use handback::HandbackWeek;
pub use prices::Cost;
pub use time::parse_date;
pub use tools::group;

use merge::Side;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
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

    /// Cache writes of both TTLs.
    pub fn cache_writes(&self) -> u64 {
        self.cache_write_5m + self.cache_write_1h
    }

    /// Share of the context served from the cache, or `None` for a request without context.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let context = self.context();
        (context > 0).then(|| self.cache_read as f64 / context as f64)
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
    /// Context size before each compaction, in transcript order.
    pub pre_compact: Vec<u64>,
    pub cost: Cost,
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
    pub tokens: Tokens,
    pub cost: Cost,
    pub cache: CacheStat,
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
    pub cost: Cost,
    /// Cost per model, in the order of `models`.
    pub model_costs: Vec<(String, Cost)>,
    pub cache: CacheStat,
    /// Compactions counted in the report.
    pub compactions: u64,
    /// Context size before the compactions: p50, p90, p99.
    pub compaction_context: [u64; 3],
    /// Spawns and hand-backs per ISO week, see `handback`.
    pub handbacks: Vec<HandbackWeek>,
}

/// Cache rewrites of a role, or of all requests. `expired` counts requests that wrote cache after
/// a gap to the previous request of their thread above the TTL; `busts` counts requests within
/// the TTL whose cache writes were over half their context.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct CacheStat {
    pub expired: u64,
    /// List price of the cache writes of the expired requests.
    pub expired_usd: f64,
    pub busts: u64,
}

impl CacheStat {
    fn add(&mut self, o: &CacheStat) {
        self.expired += o.expired;
        self.expired_usd += o.expired_usd;
        self.busts += o.busts;
    }
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
    /// Agent calls made in this file that name a subagent type.
    pub spawns: Vec<SpawnCall>,
    pub requests: Vec<Request>,
    /// Compactions of this thread, with the context size before each.
    pub compactions: Vec<Compaction>,
    pub prompts: u64,
    pub calls: Vec<Call>,
    /// Result characters of results whose call is not in this file.
    pub orphan_result_chars: u64,
}

/// One API request of a thread, sent at `ts` (ms since the epoch, 0 if unknown).
pub(crate) struct Request {
    pub id: Option<String>,
    pub model: String,
    pub tokens: Tokens,
    pub ts: u64,
}

/// An Agent call that starts a subagent of the type `role`.
pub(crate) struct SpawnCall {
    pub id: String,
    pub role: String,
    pub start_ms: u64,
    /// When the call's result came back, if it did.
    pub end_ms: Option<u64>,
}

/// A compaction of a thread and the context size before it.
pub(crate) struct Compaction {
    pub ts: u64,
    pub pre_tokens: u64,
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

/// The report over every transcript `o` selects.
pub fn report(o: &UsageOpts) -> Report {
    merge::build(&read_files(o), Side::All)
}

/// The report before and after `date` (ms since the epoch, from `parse_date`), read in one pass.
pub fn compare(o: &UsageOpts, date: u64) -> (Report, Report) {
    let files = read_files(o);
    (
        merge::build(&files, Side::Before(date)),
        merge::build(&files, Side::After(date)),
    )
}

/// Reads the transcripts `o` selects and resolves the role of each subagent.
fn read_files(o: &UsageOpts) -> Vec<FileStats> {
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
        .flat_map(|f| f.spawns.iter().map(|s| (s.id.clone(), s.role.clone())))
        .collect();
    for f in files.iter_mut().filter(|f| f.role.is_empty()) {
        f.role = f
            .spawn_id
            .as_ref()
            .and_then(|id| spawned.get(id))
            .cloned()
            .unwrap_or_else(|| "subagent".into());
    }
    files
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

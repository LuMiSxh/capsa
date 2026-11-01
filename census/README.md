# census

Usage analytics for Claude Code and Codex: tokens, cost, tool calls and subagent roles from the
session transcripts. Limits, context awareness and the hook are planned in [PLAN.md](PLAN.md).

## Install

```sh
cargo install --locked --path census
```

`./install.sh census` installs the release build instead.

## Commands

`census usage` reads `~/.claude/projects` and `~/.codex/sessions` (or `CLAUDE_CONFIG_DIR` and
`CODEX_HOME`) and prints a summary.

| Option | Meaning |
|---|---|
| `--days N` | Only transcripts modified in the last N days |
| `--project TEXT` | Only sessions whose working directory contains TEXT |
| `--top N` | Rows per table (default 10) |
| `--claude-dir DIR` | Claude Code projects directory |
| `--codex-dir DIR` | Codex sessions directory |
| `--tools` | How agents search, read, edit and build, per category |
| `--guard` | `acus guard` decisions per rule and the escaped calls, from acus's `guard.jsonl` |
| `--agents` | One row per role: spawns, models, requests, cost, cache hit rate and cache rewrites |
| `--handbacks` | Spawns per ISO week and the hand-back heuristic |
| `--compare YYYY-MM-DD` | The report before and after a UTC date, side by side with the change |

Cost is the list price of each request (`src/usage/prices.rs`): input, output and cache read at the
model's rates, cache writes at 1.25x (5 minutes) or 2x (1 hour) the input price. Haiku 5.5 requests
above 100k context tokens use its long-prompt tier. A model without a price shows `n/a` and counts
as an unpriced request.

A thread is one transcript file: the main session or one subagent. Cache hit rate is
`cache_read / (input + cache_read + cache writes)`. A request that writes cache after a gap above the
TTL to the previous request of its thread (1 hour for the main thread, 5 minutes for a subagent) is an
expired-cache rewrite. A request within the TTL whose cache writes are over half its context is an
unexpected bust; busts right after a compaction are included.

Hand-backs are a heuristic: a senior-worker spawn that starts within 30 minutes after a worker spawn
of the same session ended (or started, if its end is unknown) counts as one. The rate is hand-backs
per worker spawn, per ISO week of the spawn's start.

`--compare` splits the requests at midnight UTC of the date, by request time. A subagent counts as a
spawn on each side where it has requests; hand-backs go by spawn start. Compactions are the
`compact_boundary` records of Claude Code transcripts, with the context before each taken from
`compactMetadata.preTokens`.

Output is agent text by default; `--human` adds colour on a terminal, `--json` prints the report as
JSON. `--format agent|human|json` is the same choice.

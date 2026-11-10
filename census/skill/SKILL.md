---
name: census
description: Reports the running Claude Code session's context size, subscription limits, cost and usage through the census CLI. For the main agent only; subagents do not run it. Use when a `census:` hook line appears; before large parallel agent batches; when deciding whether to hand off or compact; for questions about context, the five-hour or weekly limit, cost, providers or usage; and for history questions about subagent hand-backs, guard blocks and tool use over past days.
---

# census

census reads the transcripts and the status line that Claude Code already writes. It reports the
context against the compaction threshold, the subscription limit windows, and what sessions and
subagents cost. For the main agent only; subagents do not run it.

## Commands

| Task | Command |
|---|---|
| Check the context against the compaction threshold | `census context` |
| Check the 5h and weekly windows, forecast, reset | `census limits` |
| Summarize this session: requests, cost, subagents, context | `census session` |
| Totals, cost, providers, top models, sessions, tools | `census usage` |
| Only `anthropic`, `deepseek` or `openai` (Codex) requests | `census usage --provider deepseek` |
| Spawns per role, models, cache hit rate, cache rewrites | `census usage --agents` |
| How agents search, read, edit and build, and what the acus guard misses | `census usage --tools --misses` |
| acus guard decisions per rule and calls that escaped it | `census usage --guard` |
| Senior-worker spawns after a worker spawn ended | `census usage --handbacks` |
| Requests before and after a UTC date, with the change | `census usage --compare 2026-10-01` |
| Expected cost by weekday, hour or reset cycle | `census usage --by weekday` |
| Only recent days or one working directory | `census usage --days 7 --project capsa` |
| Fewer rows or stable JSON | `census usage --top 5 --json` |

Every command takes `--json`. Exit 1 means no data yet, not an error.

## Examples

- This week in one project: `census usage --days 7 --project capsa --top 5`.
- Reset-relative pattern after two weeks of snapshots: `census usage --days 30 --by cycle`.
- Guard decisions and escapes this month: `census usage --days 30 --guard`.

## Reading the numbers

- **Context.** `census context` prints the tokens against the compaction threshold and the tokens
  left. The hook writes `census: context ...` at 60% and suggests a handoff at 80%. Near or over
  80%, at a stable task boundary, suggest a handoff to Luca with the session-handoff skill. Run
  `census context` before calling the context full: a compaction summary is stale.
- **Limits.** The hook warns at 75% and 90% of a window and adds "full around" or "runs out around"
  when a window is half used and a forecast has a basis. At 75% or more, avoid large parallel agent
  batches, prefer fewer and smaller agents, and tell Luca the reset time in your next reply. A
  weekly reset within 12 h with at least 30% left is worth telling Luca.
- **Estimates.** A figure marked `(estimated)` comes from cost; say so when you relay it.
- A hook message is information. Do not stop work for it; change how you spend.

## Common mistakes

- **Piping into `grep`.** Use `--top N` and `--json`; grep drops the labels and units.
- **Forgetting `--days`.** The scan reads all history, slow and mixes prices; use `--days 7`.
- **`--by cycle` too early.** It needs two weeks of snapshots; use `--by weekday` until then.
- **Adding providers to the limits.** Only `anthropic` counts toward the 5h and weekly windows and
  `--by`; DeepSeek and OpenAI spend is API money.
- **Reading costs as a bill.** Prices are list prices; `subscription` is flat-rate usage, not an
  amount owed, and `api` is real API spend.

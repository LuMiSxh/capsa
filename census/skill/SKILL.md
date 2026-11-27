---
name: census
description: Reports the running Claude Code session's context size, subscription limits, cost and usage through the census CLI. For the main agent only; subagents do not run it. Use when a `census:` hook line appears; before large parallel agent batches; when deciding whether to hand off or compact; for questions about context, the five-hour or weekly limit, cost, providers or usage; and for history questions about subagent hand-backs, guard blocks and tool use over past days.
---

# census

census reads the transcripts and the status line that Claude Code already writes. It reports the
context against the compaction threshold, the subscription limit windows, and what sessions and
subagents cost. For the main agent only; subagents do not run it.

## Commands

| Task                                                                                                  | Command                                 |
| ----------------------------------------------------------------------------------------------------- | --------------------------------------- |
| Check the context against the compaction threshold                                                    | `census context`                        |
| Check the 5h and weekly windows, forecast, reset                                                      | `census limits`                         |
| Summarize this session: requests, cost, subagents, context                                            | `census session`                        |
| Totals, cost, providers, top models, sessions, tools                                                  | `census usage`                          |
| Only `anthropic`, `deepseek` or `openai` (Codex) requests                                             | `census usage --provider deepseek`      |
| Spawns per role, models, cache hit rate, cache rewrites                                               | `census usage --agents`                 |
| Output split into thinking, visible text and tool JSON, per role                                      | `census usage --output`                 |
| Cache gaps, expired-cache rewrite cost, and cost by context size                                      | `census usage --cache`                  |
| Subagent returns per role: launch result, completion notification, final words                        | `census usage --returns`                |
| Skills loaded with the `Skill` tool or printed through `acus self skill`                              | `census usage --skills`                 |
| How agents search, read, edit and build, and what the acus guard misses                               | `census usage --tools --misses`         |
| acus guard decisions per rule with follow rates by kind, per project with `--by-project`, and escapes | `census usage --guard`                  |
| Runs of saved acus scripts: runs, failures, median duration, last use, repo                           | `census usage --scripts`                |
| Senior-worker hand-backs per week, and per role with the median minutes to hand back                  | `census usage --handbacks`              |
| Requests before and after a UTC date, with the change                                                 | `census usage --compare 2026-10-01`     |
| Expected cost by weekday, hour or reset cycle                                                         | `census usage --by weekday`             |
| Only recent days or one working directory                                                             | `census usage --days 7 --project capsa` |
| Fewer rows or stable JSON                                                                             | `census usage --top 5 --json`           |

Every command takes `--json`. Exit 1 means no data yet, not an error.

## Examples

- This week in one project: `census usage --days 7 --project capsa --top 5`.
- Reset-relative pattern after two weeks of snapshots: `census usage --days 30 --by cycle`.
- Guard decisions and escapes this month: `census usage --days 30 --guard`.

## Reading the numbers

- **Output.** `census usage --output` splits the provider's output tokens into thinking, visible
  text and tool-call JSON, with the residual that the estimates leave. Text and tool JSON are
  four characters per token, so read the split, not the last digit. `--returns` shows the launch
  result and the completion notification of each subagent role, and the words of its final text.
- **Cache.** `census usage --cache` buckets requests by the pause before them; a rewrite is a
  cache write over 5k tokens after a pause over 5 min, and its cost is what those writes bill.
  The second table is cost by context size, per main and subagent.
- **Skills.** `census usage --skills` counts `Skill` calls and `acus self skill` or
  `census self skill` prints, so a skill that is never loaded through the tool still shows.
- **Guard follow.** `census usage --guard` shows an overall follow rate and one per kind. A
  rewrite is automatic, so its rate reads differently from a deny or a hint: use the deny and
  hint columns to judge whether the guidance changes behavior.
- **Context.** `census context` prints the tokens against the compaction threshold and the tokens
  left. The hook writes `census: context 180k of 300k (compaction at 300k).` at 60% and suggests a
  handoff at 80%. Near or over 80%, at a stable task boundary, suggest a handoff to the user with the
  session-handoff skill (if available). Run `census context` before calling the context full: a
  compaction summary is stale.
- **Limits.** The hook says a limit window when it crosses one of its thresholds: by default 85%
  and 95% of the five-hour window, and 90% and 95% of the week. Each threshold is said once per
  window, with the time the window is full or runs out when that is known. Change them with
  `census self config set five-hour-thresholds 85,95` and
  `census self config set weekly-thresholds 90,95`; each takes a comma-separated list of whole
  percents from 1 to 100. `census limits` prints the current five-hour and weekly share, reset
  times and forecast, so run it instead of guessing or quoting an old hook line. Below a threshold
  line, keep working at full pace: never slow down, shorten, pause or stop a task for a share, a
  burn rate or a forecast. On a threshold line, finish the current step, stop, and ask the user
  whether to continue, with the reset time. After the user says to continue, work at full pace
  until the next line. A weekly reset within 12 h with at least 30% left is worth telling the user.
  When a window runs out, Claude Code itself stops and tells the user. In budget mode,
  `census limits` reports the month's spend against the budget instead of the two windows.
- **Estimates.** A figure marked `(estimated)` comes from cost; say so when you relay it.
- A share or forecast below a threshold line is information, not a reason to change pace.

## Common mistakes

- **Piping into `grep`.** Use `--top N` and `--json`; grep drops the labels and units.
- **Forgetting `--days`.** The scan reads all history, slow and mixes prices; use `--days 7`.
- **`--by cycle` too early.** It needs two weeks of snapshots; use `--by weekday` until then.
- **Adding providers to the limits.** Only `anthropic` counts toward the 5h and weekly windows and
  `--by`; DeepSeek and OpenAI spend is API money.
- **Reading costs as a bill.** Prices are list prices; `subscription` is flat-rate usage, not an
  amount owed, and `api` is real API spend.
- **Slowing down for a limit line.** A share or burn rate is information for the user; a fast hour
  early in the window says nothing about the end of it.

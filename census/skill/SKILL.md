---
name: census
description: Context size, subscription limits and usage of the running Claude Code session via the census CLI. For the main agent only; use when a "census:" hook message appears, before large parallel agent batches, or when asked about context, limits or cost.
---

# census

census reads the transcripts and the status line of Claude Code and reports how full the context is,
how much of the subscription limits is used, and what sessions cost. It is for the main agent.
Subagents do not need it and should not run it.

## Commands

| Command | Use it to |
|---|---|
| `census context` | See the context tokens against the compaction threshold, with the share and tokens left. Run it before a long task or when a handoff is in question. |
| `census limits` | See the 5h and weekly windows: share used, reset time, when each fills at the current rate, the weekly forecast. Run it before launching many parallel agents. |
| `census session` | See duration, requests, cost, subagents per role, context and compactions of this session. |
| `census usage` | Token, cost and tool-call statistics over all transcripts (`--days N`, `--agents`, `--tools`). Rarely needed in a task. |

All take `--json`. Exit 1 means no data yet, not an error.

## Hook messages

A hook adds a line starting with `census:` to your context when a level is crossed. Each appears once
per session or window. Times are those of `census limits`.

- `context 245k of 400k`: information only. Plan work so it finishes cleanly.
- `context 325k of 400k ... suggest a handoff`: at the next stable task boundary (not mid-edit,
  not with a failing build), suggest a handoff to Luca and use the session-handoff skill if he agrees.
- `5h limit 82%` or `7d limit 91%` (also "full around" or "runs out around"): avoid large parallel
  agent batches, prefer fewer and smaller agents, and tell Luca the reset time in your next reply.
- `weekly limit resets in 9 h with 40% left`: tell Luca, so he can use the unused share.
- `census: status: context 48k of 400k (compaction at 400k); 5h 8% (reset ...).`: not a level. It comes
  with the first prompt of a session and after an hour without one. Information only.

Before you say the context is full or suggest a handoff for context reasons, run `census context`.
Statements about context size in a compaction summary are outdated.

A figure marked `(estimated)` is computed from cost, not read from the status line; say so when you
relay it. Do not stop work because of a message; adapt how you spend.

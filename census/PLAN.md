# census plan

census is a new capsa tool for usage analytics and for making the main agent of
a Claude Code session aware of its own context size and of the subscription's
5-hour and weekly limits. It takes over `acus usage`. Status: planned
2026-10-31, nothing built yet.

## Scope

| Command | Purpose |
|---|---|
| `census usage` | Transcript analytics, moved from `acus usage` and extended (see below) |
| `census limits` | 5-hour and weekly limit: current estimate, burn rate, forecast, reset time |
| `census context` | Context size of the current session against the auto-compaction threshold |
| `census session` | Summary of the running session for the main agent: cost so far, spawns per role, context, compactions |
| `census statusline` | `statusLine` command: records a limit snapshot; prints nothing by default (Luca watches usage on claude.ai) |
| `census prune` | Deletes history: `--before DATE` or `--keep DAYS` for hourly aggregates and snapshots, `--dry-run` shows what would go |
| `census hook` | `UserPromptSubmit` and `PostToolUse` hook: injects warnings into the main agent only |
| `census recall 'PAT'` | Later: search the full transcript of a session, including the part before compaction |
| `census self skill` | Prints or installs census's own small skill |

CLI rules as in the rest of capsa: `<tool> <action>`, `<tool> self <action>`,
`--format agent|human|json`, agent format unless `CAPSA_HUMAN=1`.

Out of scope: dashboards, weekly reports, the unofficial OAuth usage endpoint
(`/api/oauth/usage`) and `anthropic-ratelimit-unified-*` headers.

## Data sources

| Data | Source | Terminal | Zed (claude-agent-acp) |
|---|---|---|---|
| Context size | `usage` of the last assistant message in `transcript_path` (input + cache read + cache creation) | yes | yes, the adapter writes transcripts too |
| Compaction threshold | `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, else `autoCompactWindow` from settings (currently 400000), capped at the model window; `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` lowers it | yes | yes |
| 5h and weekly limit | statusLine JSON `rate_limits.five_hour` / `rate_limits.seven_day` (`used_percentage`, `resets_at`), documented at code.claude.com/docs/en/statusline | yes | via pons: claude-agent-acp forwards the SDK's `rate_limit_event` as `usage_update` with `_meta["_claude/rateLimit"]` (`rateLimitType`, `utilization`, `resetsAt`, `status`); pons appends it to the same snapshot file |
| Main agent vs subagent | hook input `agent_id` is present only inside a subagent | yes | to verify |

Limits are per account, so a snapshot written by any terminal session or by
pons is valid for every session on the same Mac.

Snapshot format: one JSON object per line in
`~/.local/state/census/snapshots` (`{"t":…,"source":"statusline|pons","window":"five_hour|seven_day","used_percentage":…,"resets_at":…}`).
The format lives in `capsa-core` (a small `census_snapshot` module with the
path and the line type), so pons writes it without depending on census.

Verified 2026-11-01 on Claude Code 2.1.308: `rate_limits` is present from the
first API response on; `agent_id` is set exactly for subagent hook calls
(`PostToolUse` fires for subagent tools too); transcript `usage` equals
`context_window.total_input_tokens`; an empty statusLine still reserves a
blank row. claude-agent-acp 0.85.0 loads user hooks (`settingSources`
includes `user`). Still open: how often the SDK emits `rate_limit_event` under
the adapter, and whether `utilization` is 0–1 or 0–100 (check with a pons
debug log before relying on it).

## Ledger: incremental transcript index

No daemon and no second binary. census keeps an incremental index that every
`census` call brings up to date before it answers:

- State in `~/.local/state/census/`: per transcript file the inode, size and
  byte offset already read, plus `ledger` rows for every API request:
  timestamp, session id, agent id and role, model, token classes (input, cache
  read, 5m cache write, 1h cache write, output), and the gap to the previous
  request of the same agent thread.
- Requests are deduplicated by message id (one assistant message can span
  several transcript records with the same `usage`).
- Only new bytes are parsed. A rewritten or truncated file (size below offset,
  inode changed) is rescanned from the start.
- Raw rows are kept for 8 days (the weekly window needs 7). Before a raw row
  is pruned it is folded into an hourly aggregate (cost and tokens per hour,
  model family and role) that is kept indefinitely; a year is about 9k rows.
  Snapshots are kept indefinitely as well. `census prune` removes old
  aggregates and snapshots on request; raw rows within the 8 days are never
  pruned by it, since the open windows need them.
- One writer at a time via a lock file; a caller that cannot get the lock
  within a few milliseconds uses the ledger as it is.
- Budget: the hook path must stay well under 100 ms on a warm ledger.
- Covers Claude Code transcripts (terminal and Zed). Codex has its own quota
  and stays out of the limit math; `census usage` still reads Codex sessions.

## Limits and burn rate

Each snapshot (from `census statusline`) is appended to
`~/.local/state/census/snapshots` with time, window, `used_percentage` and
`resets_at`.

Calibration per window (5h, 7d):

1. Window start = `resets_at` minus the window length.
2. `cost_window(t)` = list-price cost of all ledger rows between window start
   and `t`.
3. Each snapshot gives one calibration point `k = used_percentage / cost_window`.
   `k` is the median of recent points of the same window (robust against
   single outliers).
4. Estimate without a fresh snapshot: `pct_now = k * cost_window(now)`.
   Works between snapshots and in Zed.
5. Burn rate = cost per minute over the last 15 and 60 minutes; forecast
   `minutes_to_full = (100 - pct_now) / (k * rate)`, compared with `resets_at`.
6. `census limits` reports the estimate, the last snapshot and its age, the
   calibration error (spread of `k`), and the forecast, e.g.
   `5h 62% (est., snapshot 12 min old), at this rate full 17:20, reset 18:40`.

### Usage pattern

Habits follow the weekly cycle. Example: the weekly limit resets early on
Sunday, and on the Sunday before the reset Luca uses up two or three 5-hour
windows to spend what is left. A plain recent-rate forecast misses that, so
the forecast also uses the history:

- Expected cost per bucket, from the hourly aggregates. Two bucket keys:
  weekday and hour of day, and hours since the last weekly reset (the reset
  time drives the behaviour more than the calendar does).
- Recent weeks count more: exponential weighting with a half-life of about
  four weeks, so a change in habits takes over within a month. Weeks with a
  different reset time are mapped by hours since reset.
- Weekly forecast: `pct_now` plus the expected cost until `resets_at` from the
  pattern, blended with the current rate for the next hour or two. Result is
  one of: "runs out around Saturday 21:00" or "ends at about 30 % unused".
- 5-hour windows: expected number of windows per day and how full they
  usually get, so "today is a typical Sunday, three windows ahead" is
  visible.
- `census limits` shows the current state first (estimate, snapshot age,
  reset times), then the forecast, then the pattern it used.
- `census usage --by weekday|hour|cycle` prints the history as a table, for
  tuning the levels and the forecast.

Known error sources, shown as part of the calibration error rather than
hidden:

- Usage outside this Mac's transcripts: the other Mac (Mina/Mara share the
  account), claude.ai web/desktop/mobile. Each fresh snapshot recalibrates.
  Later option: pull the other Mac's ledger over `ssh` (`ssh -J` fallback).
- Anthropic's internal weighting per model is unknown; list prices are the
  first approximation. If the spread of `k` stays large, fit one coefficient
  per model family from the snapshot history (`census limits --fit` reports
  the residual error).

## Hook (`census hook`)

- Registered for `UserPromptSubmit` and `PostToolUse` (the latter so long
  autonomous runs are covered too).
- Prints nothing when the input has `agent_id` (subagent). Only the main agent
  gets output.
- Always exits 0, never blocks, hard internal timeout; any error means no
  output.
- Emits `additionalContext` only when a level is crossed, once per level per
  session (state in `~/.local/state/census/hints/<session>`):
  - Context at 60 % and 80 % of the compaction threshold:
    `Context 245k of 400k (compaction at 400k). At the next task boundary, suggest a handoff to Luca (session-handoff skill).`
  - 5h or weekly limit at 75 % and 90 %, and when the forecast says the
    window fills before it resets:
    `5h limit 82% (est.), full around 17:20, reset 18:40. Avoid large parallel agent batches; tell Luca.`
- Weekly forecast: once per weekly cycle when the pattern says the weekly
  limit runs out before the reset, and once when the reset is near with a
  large unused share (`Weekly resets in 9 h with 40 % left.`), so the agent
  can tell Luca in both directions.
- Text stays far below the 10,000 character `additionalContext` cap.

## `census usage` improvements

Everything `acus usage` does today (`--tools`, `--guard`, `--agents`, `--days`,
`--project`, `--top`, formats) moves unchanged first; then:

| Addition | Detail |
|---|---|
| Cost in $ per model | Price table per model (input, output, cache read, 5m and 1h write multipliers; Haiku 5.5 long-prompt tier above 100k). Weighted tokens stay as a column. |
| Hand-back rate | worker spawns followed by a senior-worker spawn in the same parent session (heuristic, labelled as such), per week |
| Cache hit rate per session and role | `cache_read / (input + cache_read + cache_write)` |
| Expired-cache rewrites | Requests whose gap to the previous request of the same thread exceeds the TTL (5 min for subagents, 1 h for the main agent) and that write cache again; count and cost per role. Long tool calls in subagents (builds over 5 minutes) show up here. |
| Unexpected cache busts | Large cache writes in a thread although the gap is below the TTL (for example an effort or model change mid-session) |
| Compactions | Per session: number of compactions and context size at each compaction; handoffs vs compactions over time |
| `--compare DATE` | Same report before and after a date, to check whether a config change helped |

## Skill

Small, main agent only (no subagent lists it in `skills:`):
when to call `census context`, `census limits`, `census session`, and what to
do on a hook warning (handoff at a task boundary, fewer parallel agents,
tell Luca the reset time).

## Changes in acus

- Remove `acus usage` (hard move, no alias); the `acus-usage` crate moves to
  `census/`. Update `acus/README.md`, `acus/skill/SKILL.md` and any hint text
  that names `acus usage`.
- Ship together with the already finished, uncommitted removal of the
  `git log` hint (guard, tests, `acus/docs/guard.md`, CHANGELOG).
- Release as acus 0.19.0.

## Suite integration

- Workspace member `census/`, own CHANGELOG, README and AGENTS.md; add it to
  the tool table in `CLAUDE.md`/`AGENTS.md` and to the root `PLAN.md`.
- Dependencies point to `core` only: `acus-usage` uses `acus_walk` today, so
  the walk code needed by census moves to `capsa-core` (second user), and
  `--guard` parses acus's `guard.jsonl` lines itself (`rule` is a free string)
  instead of depending on `acus-guard`.
- Keep census out of acus's guard startup path; the hook path has its own
  measured budget, recorded in census's CHANGELOG.
- `.github/workflows/release.yml`: add `census` to the `tool` options and to the
  validate, build and release case blocks.
- `install.sh` / `install.ps1`: add census.
- Local install like the other tools: `cargo install --locked --path census`.

## MacSet

Already decided, one commit together with Luca's pending edits
(`agent/claude/CLAUDE.md` without the Macs section, agents effort and
indentation, `settings.json` reformat):

- `agent/claude/output-styles/luca.md`: base the delegation rule on cost (every
  request re-sends the conversation at main-model prices) instead of
  "protecting the context window". Rules themselves unchanged.
- `agent/skills/session-handoff/SKILL.md`: end with `/rename <name>` for the
  current session and `claude --resume <name>` to come back.
- No secret deny rules (decided against).

After census exists:

- `agent/claude/settings.json`: `statusLine` → `census statusline`; hooks
  `UserPromptSubmit` and `PostToolUse` → `census hook` (with the same
  `|| $HOME/.local/bin/... || true` fallback as the acus hooks).
- Deploy to Mina and Mara.
- Pending from before: add `--no-notice` to Mara's Zed agent entry line by
  line (her `ssh_connections` must stay).

## Verification before building

1. Does the current Claude Code build fill `rate_limits` in the statusLine
   JSON? Two issues report it missing or `null` in 2.1.138 to 2.1.142. Test
   with a statusLine script that dumps its input.
2. Do user hooks (`UserPromptSubmit`, `PostToolUse`) and `agent_id` fire under
   claude-agent-acp in Zed? Check with a logging hook and the pons logs. If
   not, census in Zed is limited to `census context` / `limits` on request,
   and pons could inject the warning instead (separate decision).
3. Does the transcript `usage` of the last assistant message match the
   statusLine `context_window.total_input_tokens`?

## Order

1. MacSet edits above, one commit, deploy to both Macs.
2. Verification 1 to 3.
3. census skeleton, move `acus usage`, acus 0.19.0.
4. Ledger.
4b. pons: write limit snapshots from `usage_update` `_meta["_claude/rateLimit"]`
   (pons change; no effect on the forwarded message).
5. `census usage` additions.
   5b. Evaluate per role (hand-back rate, expired-cache rewrites, tool mix,
   tokens per spawn; `--compare 2026-10-31` for the effort changes) and
   propose targeted edits to the subagent prompts in MacSet
   `agent/claude/agents/*.md`. No blind rewrite.
6. `census context`, `census statusline`, `census limits` with calibration and
   forecast, `census session`.
7. `census hook` (senior-worker: runs on every prompt and must never block).
8. Skill, reviewer pass, release census 0.1.0.
9. MacSet: statusLine and hooks, deploy to Mina and Mara.
10. Later: `census recall`.

## Open questions

- Does Claude Code keep the statusLine row hidden when the command prints
  nothing, or does it reserve an empty row? Check with verification 1.
- Fetch the other Mac's ledger over ssh, or rely on recalibration only?

Decided: levels 60/80 % (context) and 75/90 % (limits) stay as they are and
are tuned later with real data.

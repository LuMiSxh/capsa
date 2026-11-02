# census

Usage analytics for Claude Code and Codex: tokens, cost, tool calls and subagent roles from the
session transcripts. Context awareness for the running session, and its status line, are here too;
limits are here; the hook tells the main agent when a level is crossed (design in [PLAN.md](PLAN.md)).

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
| `--by weekday\|hour\|cycle` | Expected cost per bucket of the usage pattern (see Limits) |

Cost is the list price of each request (`src/usage/prices.rs`): input, output and cache read at the
model's rates, cache writes at 1.25x (5 minutes) or 2x (1 hour) the input price. Haiku 5.5 requests
above 100k context tokens use its long-prompt tier. A model without a price shows `n/a` and counts
as an unpriced request.

A thread is one transcript file: the main session or one subagent. Cache hit rate is
`cache_read / (input + cache_read + cache writes)`. A request that writes cache after a gap above the
TTL to the previous request of its thread (1 hour for the main thread, 5 minutes for a subagent) is an
expired-cache rewrite. A request within the TTL whose cache writes are over half its context is an
unexpected bust. The first request after a compaction (its context under half of the previous
request's, or a compact boundary before it) that writes cache is a compaction instead, counted
apart, and its writes are neither expired nor bust. `<synthetic>` messages (aborts and errors) are
not requests.

Hand-backs are a heuristic: a senior-worker spawn that starts within 30 minutes after a worker spawn
of the same session ended (or started, if its end is unknown) counts as one. The rate is hand-backs
per worker spawn, per ISO week of the spawn's start.

`--compare` splits the requests at midnight UTC of the date, by request time. A subagent counts as a
spawn on each side where it has requests; hand-backs go by spawn start. Compactions are the
`compact_boundary` records of Claude Code transcripts, with the context before each taken from
`compactMetadata.preTokens`.

Output is agent text by default; `--human` adds colour on a terminal, `--json` prints the report as
JSON. `--format agent|human|json` is the same choice.

### Limits

`census limits` estimates the two subscription windows, the five-hour one and the weekly one, and
forecasts the weekly one. Cost is the list price from the ledger, as in `census usage`.

- Calibration per window: each snapshot gives one point, `used_percentage` divided by the list-price
  cost of the window from its start (`resets_at` minus the window length) to the snapshot. `k` is the
  median of the last 20 points within 30 days. Points with less than $0.01 of cost or a share of 0
  are skipped. The spread is `(p75 - p25) / k` and needs four points.
- Estimate: a snapshot under two minutes old is the reading. Otherwise the share is `k` times the
  cost since the window start. The window is the newest snapshot's (`resets_at` in the future); a
  weekly window past its reset repeats every seven days; a five-hour window past its reset starts at
  the first request after it.
- Burn rate: the list-price cost of the last 15 and 60 minutes, per hour. `full around` is the time
  the share reaches 100 % at the 60-minute rate; otherwise `lasts until reset`.
- Pattern: the list-price cost of every hour of the history, weighted by `0.5^(age / 28 days)`.
  Hours are keyed by weekday and hour, and by hours since the weekly reset once the snapshots map two
  weeks of hours to their windows. Times are local.
- Forecast: the share now plus `k` times the expected cost until the weekly reset. For the first two
  hours the expected cost is half the 60-minute rate and half the pattern. The output is
  `runs out around Sat 21:00` or `ends at about 30 % unused`.
- Five-hour windows: windows start at the first request after the previous one ended. The expected
  count is per weekday, weighted the same way. The typical peak is the median of the highest share of
  the last 20 windows in the snapshots (three or more) or else `k` times their cost.

Without a snapshot there is no calibration: only the burn rate is shown. `census usage --by` prints
the buckets of the pattern as a table: `weekday` (the expected cost of each day), `hour` (each hour
of the day, averaged over the weekdays) or `cycle` (each hour since the weekly reset, zero until the
snapshots map two weeks).

### Context, session and status line

| Command | Meaning |
|---|---|
| `census context [--session ID]` | Context tokens of the session against the compaction threshold, the share used and the tokens left |
| `census session [--session ID]` | Duration, requests, cost, subagents per role, context and compactions of the session |
| `census statusline [--line]` | Records the limit snapshots of Claude Code's `statusLine` input; `--line` prints `ctx 44k/400k · 5h 6% · wk 1%` |
| `census prune --before YYYY-MM-DD \| --keep DAYS [--dry-run]` | Deletes hourly aggregates and snapshots older than the cutoff; raw ledger rows stay |

The session is `--session ID`, else `CLAUDE_SESSION_ID`, else the most recently modified transcript
whose recorded directory is the current one, an ancestor or a descendant. The output names which
one it used. The threshold is `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, else `autoCompactWindow` in
`settings.json` under `$CLAUDE_CONFIG_DIR` (or `~/.claude`), capped at the model's window, then
lowered by `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`. With none of them it is the model's window, labelled
as an approximation.

Snapshots go to `$XDG_STATE_HOME/census/snapshots` (or `~/.local/state/census/snapshots`). A reading
identical to the last one of its window is not written again.

### Hook

`census hook` is for Claude Code's `UserPromptSubmit` and `PostToolUse` events. It reads the hook JSON
on stdin (at most 1 MB) and prints nothing, exit 0, unless the main agent has a new level to hear
about. Input with `agent_id` (a subagent) or another event gets nothing. Output:

```json
{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"census: context 325k of 400k (compaction at 400k). At the next task boundary, suggest a handoff to Luca (session-handoff skill)."}}
```

| Level | Shown |
|---|---|
| Context 60 % / 80 % of the compaction threshold | Once per session; 80 % adds the handoff suggestion. Both show again after the context falls below 50 %. |
| 5h or weekly limit 75 % / 90 % | Once per window |
| 5h forecast to fill before its reset, once the window is 50 % used and 30 min old; weekly forecast to run out before its reset, once it is 50 % used and two weeks of history are mapped | Once per window |
| Weekly reset within 12 h with at least 30 % left | Once per window |
| Status line (context, 5h and weekly share with reset) on the first prompt of a session or after more than 1 h without one | Each such prompt; never with `PostToolUse` |

State is `$XDG_STATE_HOME/census/hints/<session id>` (or `~/.local/state/census/...`), one level per
line, keyed with the window's reset time; the time of the last prompt is a line too.
Errors go to one line in `hook.log` in the same directory
(moved to `hook.log.1` at 256 KB). Limits are computed on a helper thread and skipped when they take
over 150 ms; a busy ledger lock skips the ledger update.

Settings (`~/.claude/settings.json`):

```json
{
  "hooks": {
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "census hook"}]}],
    "PostToolUse": [{"hooks": [{"type": "command", "command": "census hook"}]}]
  },
  "statusLine": {"type": "command", "command": "census statusline --line"}
}
```

### Self

| Command | Meaning |
|---|---|
| `census self skill [--install]` | Prints the skill for the main agent; `--install` writes `~/.claude/skills/census/SKILL.md` |
| `census self setup [--dry-run] [--force]` | Installs the skill, then wires census into `settings.json`: the statusLine and the hooks below |
| `census self update [--check\|--force]` | Replaces the binary with the latest release, then reinstalls the skill |
| `census self uninstall [--dry-run]` | Removes the skill, then the binary; settings entries stay |

Update and uninstall need the `cmd-update` feature (default). The release build passes it explicitly.

`census self setup` writes what the Hook section shows into `settings.json` under `$CLAUDE_CONFIG_DIR`
(or `~/.claude`), and installs the skill as `self skill --install` does. An entry that already runs
census is left alone, whatever else its command line holds. A `statusLine` that census does not own
is skipped unless `--force` replaces it. Other keys and entries keep their order. The file is
replaced only when something changes, through a temporary file, and the old one is copied to
`settings.json.bak` first. Invalid JSON, or a `hooks` value or event value of the wrong type, exits 2
and changes nothing. `--dry-run` writes nothing, the skill included.

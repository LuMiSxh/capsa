# Changelog

## [Unreleased]

### Fixed

- On Windows, `census limits` and `census usage` use the local time zone for the weekly and daily
  boundaries and label it `local`, instead of UTC.
- `census self setup` writes the hook and status line commands with a quoted path that uses forward
  slashes. A backslash path was read as escapes by the Git Bash that runs them.

### Added

- Budget mode for API billing. `census config set mode budget` and `census config set
  monthly-budget <usd>` (`census config show`) write `config.json` in the census config directory;
  without the file census stays in plan mode. In budget mode `census limits` prints the spend of the
  local calendar month, the budget share, the projection to the end of the month and the budget left
  per remaining day instead of the 5h and weekly windows, `census statusline --line` shows
  `$42/$200 21% →$310` (`↑` when the projection exceeds the budget), and `census hook` warns at 80 %
  and 100 % of the budget and when the projection exceeds it.

## [0.2.0]

### Added

- `census self setup [--dry-run] [--force]` installs the skill and adds census's `statusLine` and its
  `UserPromptSubmit` and `PostToolUse` hooks to `settings.json`. Entries that already run census stay
  as they are. A `statusLine` that census does not own is kept unless `--force` replaces it. The file
  is written only when something changes, through a temporary file, and the old one is copied to
  `settings.json.bak` first. Invalid JSON exits 2 and changes nothing.

## [0.1.3]

### Fixed

- `census usage --agents` no longer counts the first request after a compaction as an unexpected
  bust. A request whose context is under half of the previous request's in its transcript, or that
  follows a compact boundary, is a compaction: it has its own `compactions` column (and
  `cache.compactions` in JSON), and its cache writes are counted neither as expired nor as busts.
- `<synthetic>` messages (aborts and errors, no API call) are no longer requests. They no longer
  count in the request totals, and they no longer restart the gap of the next request, which could
  turn an expired rewrite into a bust.

## [0.1.2]

### Changed

- The hook's forecasts are said later and less often: the 5h "full before reset" forecast needs at
  least 50 % of the window used and 30 min of it passed, and the weekly "runs out before reset"
  forecast needs at least 50 % used and two weeks of history mapped to weekly windows. The 75 % and
  90 % levels and the weekly reset note are unchanged.

### Added

- `census hook` adds a status line to the context on `UserPromptSubmit` for the first prompt of a
  session and for the first prompt after more than an hour without one (the main agent's cache
  TTL), even when no level is crossed: the context against the compaction threshold, and the 5h and
  weekly shares with their resets, each only when known. `PostToolUse` never adds it.

### Fixed

- `claude-sonnet-5-5` cache reads are priced at $0.10 per million tokens, not $0.20.
- Haiku 5.5 requests whose prompt is over 100,000 tokens (input, cache read and cache writes
  together) are priced at its long-prompt tier, $0.50 input and $2.50 output per million tokens,
  with cache reads and writes scaled alike. Shorter prompts keep $0.10 and $0.50.

## [0.1.1]

### Fixed

- The hook tells the main agent once after a compaction that the context is small again, so stale
  notes in the compaction summary do not trigger a handoff.

## [0.1.0]

### Added

- `census hook`: the UserPromptSubmit and PostToolUse hook. For the main agent only, it adds a line
  to the context once per session when the context passes 60 % (information) or 80 % (suggest a
  handoff) of the compaction threshold, when the 5h or weekly limit passes 75 % or 90 %, when the
  5h window is forecast to fill or the weekly one to run out before its reset, and when the weekly
  reset is within 12 h with at least 30 % left. Always exits 0 and prints nothing on error.
- `census self skill [--install]`, `census self update [--check|--force]`, `census self uninstall
  [--dry-run]`, and the skill `census/skill/SKILL.md`. Update and uninstall need the `cmd-update`
  feature (default; the release build sets it).
- `census limits`: the five-hour and weekly subscription windows, each with its share (from the
  newest snapshot, or estimated from a calibration over the list-price cost), reset, calibration
  spread and the time it fills at the current rate. The weekly window has a forecast from the usage
  pattern (weekday and hour, or hours since the weekly reset). Exit 1 without data.
- `census usage --by weekday|hour|cycle`: the expected cost of each bucket of the usage pattern.
- `census usage`, moved from `acus usage`.
- `census usage` cost in $ per model, session and role, from a list-price table (`src/usage/prices.rs`).
  Requests of models without a price count as unpriced.
- `census usage` cache hit rate per session and role, and expired-cache rewrites and unexpected
  busts per role (`--agents`).
- `census usage --handbacks`: spawns per ISO week and a hand-back heuristic (a senior-worker spawn
  within 30 minutes after a worker spawn of the same session).
- `census usage --compare YYYY-MM-DD`: the report before and after a UTC date, with the change.
- `census usage`: compactions per session with the context size before each one.
- `census context [--session ID]`: the context size of the current session against the auto-compaction
  threshold, with the threshold's source and the model.
- `census session [--session ID]`: duration, requests, cost, subagents per role, context and
  compactions of the current session.
- `census statusline [--line]`: records the limit snapshots of Claude Code's status line input
  (`~/.local/state/census/snapshots`); `--line` prints a line such as `ctx 44k/400k · 5h 6% · wk 1%`.
  Prints nothing and exits 0 on any error.
- `census prune --before YYYY-MM-DD | --keep DAYS [--dry-run]`: deletes hourly aggregates and
  snapshots older than the cutoff. Raw ledger rows are kept.

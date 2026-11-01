# Changelog

## [Unreleased]

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

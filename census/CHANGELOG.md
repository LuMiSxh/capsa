# Changelog

## [Unreleased]

## [0.6.0]

### Added

- `census usage --provider <PROVIDER>` keeps only the requests billed by `anthropic` (Claude),
  `deepseek` or `openai` (Codex). It filters the requests before aggregation, so the totals, models,
  sessions, providers, `--agents` and `--compare` follow it; `--by` remains `anthropic`-only.
- `census usage --tools` adds `result-p50`, `result-p90` and `result-max`: the result characters of
  a call in a category at each percentile and the largest, so a category that mixes small and huge
  results shows it.
- `census usage --tools --misses` lists the most frequent normalised commands of the categories
  that bypass acus (`git diff --cached`, `cargo test | tail`, `sed -n`, `Read`), with the calls
  that ran unguarded, the `intercepted` calls that the guard denied or rewrote, and their result
  characters, at most `--top`. A command with no unguarded call is left out. The `--json` of
  `--tools` is now an object with `tools`, plus `misses` when asked.

### Fixed

- `census context` and `census session` read `CLAUDE_CODE_SESSION_ID`, which Claude Code sets for
  the running session, before the legacy `CLAUDE_SESSION_ID`. An agent that ran census no longer
  gets the newest transcript of another chat in the same directory.
- `census context` and `census session` name the variable they used when the session came from the
  fallback: the label reads `(CLAUDE_SESSION_ID)` instead of always `(CLAUDE_CODE_SESSION_ID)`.

## [0.5.0]

### Added

- Prices come from LiteLLM's price file (`model_prices_and_context_window.json`) instead of seven
  hardcoded Claude models. Census keeps the chat models of the `anthropic`, `deepseek` and `openai`
  providers in USD per million tokens, in `prices.json` in the state directory. `census usage`,
  `census limits` and `census session` fetch the list again when the cache is older than
  `prices_refresh_days` (7 by default). A failed fetch keeps the cache and writes a warning to
  `prices.jsonl`. `census hook` and `census statusline` never fetch. Without a cache the snapshot
  built into the binary is used (`data/prices.snapshot.json`).
- `census self config set prices-refresh-days <days>` sets the refresh period, at least 1. The key
  `prices_refresh_days` in `config.json` stores it, and `census self config` shows it.
- Long-prompt tiers come from the same file, at the threshold named in the key: Haiku 5.5 still pays
  its tier above 100k prompt tokens.
- `census usage` splits the requests by provider. Each model row names its provider, and a
  `providers` line totals each one: `anthropic` is the Claude subscription, `deepseek` and `openai`
  are API spend. `--json` has `providers` (requests, tokens and cost per provider) and `models` as
  objects with `model`, `provider`, `tokens` and `cost`, which replace the `models` pairs and
  `model_costs`. A model the price list does not know is `anthropic` when its id starts with
  `claude`, else `unknown`.
- `census session` shows the cost of the Claude subscription, and an `api` row with the API spend per
  provider when a subagent ran on DeepSeek. The JSON has `api`, which is absent when empty.
- `billing` in `config.json` says how each provider bills its requests: `subscription` or `api`.
  `anthropic` is the subscription by default, and a provider the map does not list is `api`.
  `census self config set billing <provider> <subscription|api>` sets one entry. Any lowercase
  provider name is accepted; a name the price table does not list gets a warning. `census self
  config` shows the map, and its `--json` gains `billing`. `census usage` names each provider's
  billing on the `providers` row and in a `billing` column of the models table, and `census
  session` counts requests by it, so `openai` set to `subscription` moves its history from API
  spend to the subscription. The limits, the budget month and `census usage --by` still count
  `anthropic` only, whatever `billing` says, so the windows keep tracking Claude's own limits.
- `census hook` also runs at `SessionStart` and points the main agent at the usage history once
  per session: hand-backs, guard blocks, tool use and cost by day or role come from `census
  usage` (`--handbacks`, `--guard`, `--tools`, `--days`) or `census session`, not from reading
  `~/.claude` transcripts. `census self setup` adds the hook to `SessionStart`.

### Changed

- The census skill follows the skill rules: a task → command table, behaviour thresholds and common
  mistakes, a description that carries every trigger, and `skill/evals/` with trigger and task
  cases. It grows from 41 lines / 2,528 bytes to 58 lines / 3,493 bytes. A test checks every
  command and flag it names against the clap definition and that its name matches the install
  directory. The evals run through `scripts/skill-evals.py`; on Haiku, trigger recall was 3/10
  with the user's hooks and 8/10 with project settings only, and tasks 2/8 and 4/8. With
  hand-back and guard triggers in the description, and a pass for an answer that quotes the
  status hook's figures without a tool call, trigger recall is 7/10 and 10/10 and tasks 4/8
  and 7/8.
- `census usage --by cycle` prints only the note when fewer than two weeks of weekly snapshots
  map the reset, instead of the 168 zero buckets. `--json` keeps its rows.
- `census session` labels its cost row `subscription` in the agent and human formats. `--json`
  keeps `cost`, which now means the requests billed as subscription under `billing`.
- `census usage` gains a `billing` column in the models table; its `--json` is unchanged.
- Models without a price in the old table now have one: `claude-haiku-4-5-20251001`, and the OpenAI
  and DeepSeek models of Codex transcripts, which showed `n/a`. On this machine's full history,
  `census usage --json` moves from $756.92 with 29,596 unpriced requests to $1,925.44 with none.
  Most of the rise is the Codex (OpenAI) requests. The Claude models priced before cost the same
  to the cent: `claude-haiku-5-5` $53.30, `claude-opus-5-5` $430.67, `claude-sonnet-5-5` $272.94.
- `claude-fable-5` cache reads cost $1.00 per million tokens, the price LiteLLM lists today; census
  had $0.25. No golden file or cost on this machine's history changed for it.
- The ledger digest records the fetch time of the price list and is rebuilt when it changes, once
  after the upgrade.
- The `self_config` and `self_config_budget` goldens gain the `prices_refresh_days` key.
- A failed price fetch is not retried for an hour: `prices.attempt` in the state directory holds the
  time of the failure, and a successful fetch clears it.
- The limits, the budget month and `census usage --by` count only requests billed to `anthropic`.
  DeepSeek subagents were priced at DeepSeek's list price and counted toward the five-hour and weekly
  windows, the burn rate, the forecast and the monthly budget. Codex (OpenAI) requests never reached
  the limits, since the ledger reads only the Claude projects. On this machine's full history, the
  providers are `anthropic` $761.83, `deepseek` $0.05 and `openai` $1,168.28 of $1,930.15.
- The ledger digest has a `format` field (2). A digest without it is rebuilt once after the upgrade,
  as the price-list change did.
- The `usage` and `usage_compare` goldens change: the `models` rows become objects with a `provider`,
  `model_costs` folds into them, and `providers` is added. `session.json` is unchanged; the `api` key
  appears only with API spend.

### Performance

- `census hook`, mean of 50 runs after a warm-up on the real history (202 transcripts, 951 MB, the
  session transcript 1.9 MB), in two rounds: before 11.28 ms and 11.61 ms; after with the snapshot
  11.16 ms and 11.48 ms; after with the cache 11.23 ms and 11.90 ms. No change beyond the noise.
  The hook reads the table once per process to check the digest's price list.
- The release binary is about 3.0 MB before and 3.1 MB after (the table and the snapshot).
- `census hook`, mean of 50 runs after a warm-up, interleaved with the other build and each on its own
  state directory, on a busy machine: before 13.93 and 13.38 ms, after 14.05 and 12.83 ms, no change
  beyond the noise. The provider filter runs when the digest is built, so the hook path reads the
  same digest as before.

## [0.4.0]

### Added

- The ledger keeps `digest.json` next to `rows.jsonl`, a snapshot of the priced requests and hours
  with the census version, the time of its update and the sizes of the two files it was built from.
  `census hook` and `census limits` read it instead of the whole ledger. A missing, stale or unreadable
  digest is rebuilt from the files, and `census prune` removes it.
- `census self config set daily-limit <USD|auto>` sets the daily limit of budget mode: a fixed amount, or
  `auto` (the default), which shares what is left of the monthly budget before today over the local
  days left, today included. `daily_limit_usd` and `daily_thresholds` in `config.json` store the limit
  and the announced percentages (50, 80 and 100 by default).
- The hook announces the daily budget at each threshold once per local day, and past 100 % at each
  further full multiple of the limit. The daily line comes first in the same output as the monthly
  budget line, and the monthly forecast is not repeated when the daily line states it.
- `census limits` and `census self config` show today's spend against the daily limit in budget mode.
  The JSON of `census --json limits` has a `today` object with `usd`, `limit` and `source`.
- `census self doctor` checks the install, the config file, the skill, the hooks in `settings.json` and
  the state files, one line per check with a hint for each failure. `--json` is one value; exit 1 when a
  check fails, and warnings do not change the exit code.

### Changed

- `self update` works without a GitHub token; a token only lifts the rate limit.
- Dependencies updated (zip 8 → 9 for `census self update`, and the other crates to their latest releases).
- `census self setup` writes the hook and statusLine commands as bare names, `census hook` and
  `census statusline --line`, not the path of the binary, as `acus self setup` writes its hooks.
  Claude Code runs them from PATH, so census must be on PATH: `census self doctor` warns when it is
  not. An entry that runs census in another form (an absolute or quoted path, `.exe`, or the old
  `2>/dev/null ||` fallback) is replaced by the bare command. An entry in bare form is left alone.
- The human views of `census limits` and `census usage` are redesigned on `capsa_core::render`.
  `limits --human` shows each window with a 16-cell bar, its percent and its reset, and in budget
  mode the month with a bar and its forecast, then today against the daily limit. `usage --human`
  prints its tables with right-aligned numbers. The report commands (`limits`, `usage`, `context`,
  `session`, `prune`, `self config`) share the render path; their `--json` is unchanged.
- The agent text of the report commands uses `label: value` lines and comma-separated tables. The
  information is the same: budget mode keeps its `mode` line, and the forecast keeps its
  "at the month-to-date rate" wording. The burn rate and money sums no longer print `$-0.00`.
- The guard hint of `census usage --guard` names `acus self setup`, the command that installs the
  hook.
- `census self uninstall` runs through `capsa_core::update::uninstall_run`. The check still runs
  before the binary is removed, and the skill file lines still print only outside `--json`.
- `census config` moved to `census self config`, with the same subcommands and the same JSON. The old
  path is refused with exit 2. The hints that name the command now name the new path.
- `census hook` logs its errors to `hook.jsonl`, as JSON lines, moved to `hook.jsonl.old` at 256 KB. The
  old `hook.log` is left where it is and is no longer written.
- `census self update --json` prints only the report: the reinstall of the skill no longer adds its path
  line to stdout.
- `census hook` reads the end of the transcript once. It looks for the last API request in a 512 KiB
  tail, then in 4 MiB and 16 MiB tails only while the request is not found, and reports the context as
  unknown past that. It reads `settings.json` once per call.
- `census usage --guard` streams the guard log and parses a line in full only when its time is in
  range. A line that is not valid UTF-8 is skipped and the file is read on past it.
- `census hook` on a synthetic 20-day ledger and a 6.4 MB transcript (19,200 requests): median 79 ms
  before and 69 ms after over 21 runs in the same session (70 ms before in an earlier run). Output of
  `census limits`, text and JSON, is unchanged on the same state, with and without snapshots.

### Fixed

- `census self uninstall` removes the binary before the skill file, so a failed binary removal
  leaves the skill in place.
- The `census` integration tests isolate the configuration directory, so a developer's budget
  configuration no longer changes the expected status line.
- The digest parses floats exactly (`serde_json` `float_roundtrip`), so the priced values match the
  ledger files.
- `census context` and `census session` find the transcript of a directory that Claude Code
  recorded under its canonical path, such as a Windows short name or a symlinked prefix.

## [0.3.0]

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

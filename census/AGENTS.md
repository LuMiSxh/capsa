# census agent instructions

## Purpose

census reads agent transcripts and the state of a running Claude Code session: usage analytics, the
context size against the compaction threshold, a session summary, the limit snapshots that the status
line records, and the limit windows and the budget month (`census limits`). `census hook` tells the
main agent when a context, limit or daily-budget level is crossed, and at `SessionStart` points it
at the report commands for usage history. `census self` installs the skill and wires `settings.json`
(`self setup`), edits the config (`self config`), and updates and uninstalls the binary.

## Layout

Paths are relative to `census/`. census depends on `capsa-core` only, never on an `acus-*` crate.

- `src/main.rs`: the clap CLI. One `src/cmd_<name>.rs` per command: `cmd_usage.rs` (`census usage`,
  including the `--guard` reader for acus's `guard.jsonl` and `--by`), `cmd_context.rs`,
  `cmd_session.rs`, `cmd_statusline.rs`, `cmd_prune.rs`, `cmd_limits.rs` (the plan and budget views),
  `cmd_hook.rs` (stdin cap, 150 ms limits deadline, printing), `cmd_self.rs` (`self
  skill|setup|config|doctor|update|uninstall`; update and uninstall need feature `cmd-update`, on by
  default), `cmd_doctor.rs` (`self doctor`).
- `src/cmd_config.rs`: `census self config`, which shows and sets the config.
- `src/config.rs`: `config.json` in the census config directory: `mode` (`plan` or `budget`), the
  monthly budget, the daily limit, the daily thresholds, the days before the price list is
  fetched again and `billing`, how each provider is billed (`subscription` or `api`; `anthropic`
  is `subscription` by default).
- `skill/SKILL.md`: the skill `census self skill` prints, compiled into the binary.
- `src/hook/`: the decision of `census hook`, a library returning values. `mod.rs` parses the input
  and builds the output JSON, `levels.rs` decides and words the new levels, `state.rs` owns
  `hints/<session>` and `hook.jsonl`, the error log.
- `src/limits/`: the subscription limit windows, a library that returns values only. `mod.rs` is the
  API (`status`, `usage_by`, `compute`), `cost.rs` prices rows and aggregates, `calibrate.rs` the
  median share per dollar, `window.rs` the running window, `pattern.rs` the expected cost per hour
  (weekday and hour, or hours since the weekly reset), `forecast.rs` the weekly forecast and the
  five-hour summary, `clock.rs` the calendar and `when`, `month.rs` the budget month and today's
  spend, `daily.rs` the daily limit of the budget mode, `error.rs`, `tests.rs`.
- `src/usage/`: transcript analytics. `mod.rs` builds the report, `claude.rs` and `codex.rs` parse
  transcripts, `tools.rs` classifies tool calls, `prices/` holds the list prices (see Gotchas),
  `merge.rs` folds
  the files into the report, `handback.rs` counts hand-backs, `read.rs` reads files in parallel,
  `time.rs` does dates and ISO weeks, `walk.rs` lists the `*.jsonl` files.
- `src/usage_views.rs`: the table views of `census usage` (`--by`, `--agents`, `--handbacks`,
  `--compare`, `--tools`) and the formatting helpers the report commands share (`k`, `usd`, `money`,
  `rate`).
- `src/out.rs`: `Out`, the output format and colour of a command, and `field`, the `label  value`
  line of `census self setup`.
- `src/setup.rs`: `census self setup`, the skill and the hooks and statusLine in `settings.json`.
- `src/ledger/`: the incremental transcript index behind `census prune` and the rate figures.
  `mod.rs` is the API, `update.rs` and `scan.rs` read new bytes, `parse.rs` reads records,
  `model.rs` holds the row types, `query.rs` answers the reads, `fold.rs` folds old rows into hourly
  aggregates and prunes, `digest.rs` writes `digest.json`, `state.rs` owns the state files and the
  lock.
- `src/context.rs`: the compaction threshold and the context size, read from the end of the
  transcript. The hook reuses `transcript_ends`.
- `src/session.rs`: which transcript is the session, and the summary of a session with its
  subagents.
- `src/statusline.rs`: the statusLine input, its limit snapshots and the status line text.
- `data/prices.snapshot.json`: the price table built into the binary, trimmed from LiteLLM's file
  (see Commands to regenerate it).
- `src/transcript.rs`: the transcript records that the session and context views read.
- `tests/`: `report_json.rs` pins the `--json` output of the report commands against the files in
  `tests/report_json/`; `usage.rs` and `ledger.rs` over generated transcripts; `context.rs`,
  `session.rs` and `statusline.rs` for the library; `cli.rs` for the binary in a scratch home;
  `hook.rs` and `setup.rs` for the hook and `self setup`; `common/` for scratch directories.

## Commands

- `cargo test -p census`: all census tests.
- `cargo test -p census --test report_json`: the golden `--json` tests alone.
- `CENSUS_GOLDEN_UPDATE=1 cargo test -p census --test report_json`: rewrites the golden files from
  the binary. Use it only for an intended JSON change, and record that change in CHANGELOG.
- `cargo run -q -p census -- usage --help`: the options.
- `CENSUS_PRICES_SOURCE=<litellm.json> cargo test -p census --lib regenerates_the_snapshot -- --ignored`:
  rewrites `data/prices.snapshot.json`. Download the source first with `curl -fsSL
  https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json -o
  <litellm.json>`. Put the moved prices in the CHANGELOG entry.

## Rules

Shared rules: see root AGENTS.md.

- The golden files in `tests/report_json/` pin the bytes of every report command's `--json`. A
  change to them is user-visible and goes into CHANGELOG.
- The report commands print through `capsa_core::render`. `census hook` and `census statusline
  --line` are protocol output: they stay outside `render`, and their output does not change with
  the report views.
- `census statusline` exits 0 and prints nothing unless `--line` is given. It never updates the
  ledger, so it stays fast, and it appends a snapshot only when a window's reading differs from the
  last one.
- `--guard` reads acus's `guard.jsonl` as acus writes it. `rule` is a free string: names from older
  acus versions (such as `git-log`) must still parse, so do not validate rule names.
- `census hook` exits 0 on every path and prints nothing on any error or for a subagent (input with
  `agent_id`). Keep it free of `unwrap`, indexing and panics: `panic = "abort"` makes one a failed
  hook on every prompt. Limits are skipped after 150 ms. Levels are kept per session in
  `hints/<session>`, keyed with the window's `resets_at` so a new window shows them again.
- The session is `--session`, then `CLAUDE_SESSION_ID`, then the newest main transcript whose
  recorded directory is the current one, an ancestor or a descendant. Subagent transcripts are
  never chosen.

## Gotchas

- The walk does not read `.gitignore` files, which acus's walk did. Transcript directories are not
  expected to be ignored.
- The config directory of acus comes from `capsa_core::paths::config_dir("acus")`.
  `CLAUDE_CONFIG_DIR` moves the transcripts and the settings that the threshold reads.
- `census prune` never removes raw ledger rows: the 8-day windows need them.
- Budget mode: `daily_limit_usd` unset means `auto`, the daily share of what is left of the monthly
  budget. `daily_thresholds` (50, 80 and 100 % by default) are announced by the hook once per local
  day.
- The hook's errors go to `hook.jsonl`, moved to `hook.jsonl.old` at 256 KB. The old `hook.log` is
  no longer written.
- The goldens in `tests/report_json.rs` pin the local offset with `CENSUS_TZ_OFFSET_SECS=7200`
  (a fixed +02:00, so DST is deliberately not modelled), so they do not depend on the machine's zone.
  Re-record them only for an intended change. The override is read in
  `limits/clock.rs` `offset_at`: when it parses, it is the offset at every instant and wins over the
  system zone. `usage_hour_buckets_follow_the_offset` checks that it is used.
- `CENSUS_HOOK_DEADLINE_MS` replaces the hook's 150 ms limits deadline (`cmd_hook.rs`). The hook
  tests set it to 60000: under parallel load the 150 ms deadline skipped the limits and failed them.
- Prices: `usage/prices/`. `table.rs` trims LiteLLM's file to the chat models of anthropic, deepseek
  and openai, in USD per million tokens, with the long tiers and the context window. `source.rs`
  reads the cache and fetches. The table in use is `<state>/census/prices.json` when it reads, else
  the snapshot. Only `census usage`, `census limits` and `census session` refresh the cache, when it
  is older than `prices_refresh_days`; a failed fetch keeps it and writes `prices.jsonl`. `census
  hook` and `census statusline` never fetch. The digest records the table's `fetched_at`, so a new
  list reprices it.
- A model missing from the table is unpriced: `n/a` and `(+N unpriced)`, never 0.
- `CENSUS_NO_PRICE_REFRESH=1` turns the fetch off. The CLI and golden tests set it, so they price
  from the snapshot.
- `WEIGHTS` in `usage/mod.rs` is a fixed unit, not read from the table: the list ratios differ per
  model, and the weighted unit feeds the limit calibration.
- The provider of a request is `billed_by` in `usage/prices/mod.rs`: the table's provider, else
  `anthropic` for a `claude` id, else `unknown`. `billing` in the config classifies it for `census
  usage` and `census session` only (`Config::billing_of`). The limits, the budget month and `census
  usage --by` count `counts_toward_limits`, which is `anthropic` alone, whatever `billing` says: the
  digest keeps their USD and an id for every raw row, so the updater does not read a request twice.
  `FORMAT` in `ledger/digest.rs` names the digest layout;
  bump it with any change to the digest's fields or to what its USD means.
- Folded hourly aggregates carry no provider. Their family `other` has no price, so a folded row of
  another provider adds 0 USD, while a raw row of it is left out of the digest.
- Codex rows never reach the ledger: its roots are the Claude projects. They appear only in
  `census usage`, under their provider.

# census agent instructions

Suite-wide rules are in ../AGENTS.md. This file covers census only.

## Purpose

census reads agent transcripts and the state of a running Claude Code session: usage analytics,
the context size against the compaction threshold, a session summary, and the limit snapshots that
the status line records, and the limit windows (`census limits`). `census hook` tells the main agent when a context or limit
level is crossed; `census self` installs the skill, updates and uninstalls the binary.

## Layout

Paths are relative to `census/`. census depends on `capsa-core` only, never on an `acus-*` crate.

- `src/main.rs`: the clap CLI. One `src/cmd_<name>.rs` per command: `cmd_usage.rs` (`census usage`,
  including the `--guard` reader for acus's `guard.jsonl` and `--by`), `cmd_context.rs`,
  `cmd_session.rs`, `cmd_statusline.rs`, `cmd_prune.rs`, `cmd_limits.rs`, `cmd_hook.rs` (stdin cap,
  150 ms limits deadline, printing), `cmd_self.rs` (`self skill|update|uninstall`; update and
  uninstall need feature `cmd-update`, on by default).
- `skill/SKILL.md`: the skill `census self skill` prints, compiled into the binary.
- `src/hook/`: the decision of `census hook`, a library returning values. `mod.rs` parses the input
  and builds the output JSON, `levels.rs` decides and words the new levels, `state.rs` owns
  `hints/<session>` and `hook.log`.
- `src/limits/`: the subscription limit windows, a library that returns values only. `mod.rs` is the
  API (`status`, `usage_by`, `compute`), `cost.rs` prices rows and aggregates, `calibrate.rs` the
  median share per dollar, `window.rs` the running window, `pattern.rs` the expected cost per hour
  (weekday and hour, or hours since the weekly reset), `forecast.rs` the weekly forecast and the
  five-hour summary, `clock.rs` the calendar and `when`, `error.rs`, `tests.rs`.
- `src/usage/`: transcript analytics. `mod.rs` builds the report, `claude.rs` and `codex.rs` parse
  transcripts, `tools.rs` classifies tool calls, `prices.rs` holds the list prices, `merge.rs` folds
  the files into the report, `handback.rs` counts hand-backs, `read.rs` reads files in parallel,
  `time.rs` does dates and ISO weeks, `walk.rs` lists the `*.jsonl` files.
- `src/usage_views.rs`: the printed views of `census usage`.
- `src/ledger/`: the incremental transcript index behind `census prune` and the rate figures.
  `mod.rs` is the API, `update.rs` and `scan.rs` read new bytes, `parse.rs` reads records,
  `fold.rs` folds old rows into hourly aggregates and prunes, `state.rs` owns the state files and
  the lock.
- `src/context.rs`: the compaction threshold and the context size, read from the end of the
  transcript. The hook reuses `of_transcript`.
- `src/session.rs`: which transcript is the session, and the summary of a session with its
  subagents.
- `src/statusline.rs`: the statusLine input, its limit snapshots and the status line text.
- `src/transcript.rs`: the transcript records that the session and context views read.
- `src/out.rs`: section headers and `label  value` fields, coloured only on a terminal.
- `tests/`: `usage.rs` and `ledger.rs` over generated transcripts; `context.rs`, `session.rs` and
  `statusline.rs` for the library; `cli.rs` for the binary in a scratch home; `common/` for scratch
  directories.

## Commands

- `cargo test -p census`: all census tests.
- `cargo run -q -p census -- usage --help`: the options.

## Contracts

- `census usage` output, text and `--json`, is the same as `acus usage` had. A change to it is a
  user-visible change and goes into CHANGELOG.
- `--guard` reads acus's `guard.jsonl` as acus writes it. `rule` is a free string: names from older
  acus versions (such as `git-log`) must still parse, so do not validate rule names.
- `census statusline` exits 0 and prints nothing unless `--line` is given. It never updates the
  ledger, so it stays fast, and it appends a snapshot only when a window's reading differs from the
  last one.
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

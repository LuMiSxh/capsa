# census agent instructions

Suite-wide rules are in ../AGENTS.md. This file covers census only.

## Purpose

census reads agent transcripts and, in later steps, the subscription limits and context size of a
running session. Today it has `census usage`, moved unchanged from `acus usage`.

## Layout

Paths are relative to `census/`. census depends on `capsa-core` only, never on an `acus-*` crate.

- `src/main.rs`: the clap CLI. One `src/cmd_<name>.rs` per command; `src/cmd_usage.rs` holds
  `census usage`, including the `--guard` reader for acus's `guard.jsonl`.
- `src/usage/`: transcript analytics. `mod.rs` builds the report, `claude.rs` and `codex.rs` parse
  transcripts, `tools.rs` classifies tool calls, `walk.rs` lists the `*.jsonl` files.
- `src/out.rs`: section headers, coloured only on a terminal.
- `tests/usage.rs`: report tests over generated transcripts.

## Commands

- `cargo test -p census`: usage tests.
- `cargo run -q -p census -- usage --help`: the options.

## Contracts

- `census usage` output, text and `--json`, is the same as `acus usage` had. A change to it is a
  user-visible change and goes into CHANGELOG.
- `--guard` reads acus's `guard.jsonl` as acus writes it. `rule` is a free string: names from older
  acus versions (such as `git-log`) must still parse, so do not validate rule names.

## Gotchas

- The walk does not read `.gitignore` files, which acus's walk did. Transcript directories are not
  expected to be ignored.
- `config_dir` in `src/cmd_usage.rs` copies acus's config directory rule; keep them in step.

# Audit: core (`capsa-core`)

Read-only audit from 2026-11-11. Line counts are raw file lines including tests. Work order and
owners live in [AUDIT.md](AUDIT.md); the step ids below point there.

## State

- 7.1k lines over 14 modules. No file over 500 code lines (largest `claude_settings/mod.rs` 441),
  no function over 100 lines.
- Used by every tool. `exit`, `format`, `hint` and `render` are adopted everywhere;
  `census_snapshot` is used by census and pons, `pkg` by pons only, `claude_settings::*_env` by
  vicarius only.

## Findings

| # | Finding | Evidence | Step |
|---|---------|----------|------|
| C1 | Atomic write (temp + rename) is implemented 7 times across the workspace; core has no shared helper. | `claude_settings/mod.rs:181-213`, `census_snapshot.rs:129-150`, `update/binary.rs:94-103`; census `ledger/state.rs:93`, `usage/prices/source.rs:121`; vicarius `config.rs:281-293`, `agents/sync.rs:121` | 1 |
| C2 | `log::read` returns a bare `Vec` and drops I/O and parse errors; a caller cannot tell "no log" from "corrupt log". | `log/mod.rs:73-86` | 1 |
| C3 | `paths::expand_home(&str) -> String` while `config_dir`/`state_dir` return `PathBuf`; callers rewrap. | `paths.rs:64`, `acus-cli/src/cmd_find/mod.rs:239` | 1 |
| C4 | "NotFound means empty" match and an `io_err` closure are repeated. | `census_snapshot.rs:70,102,106-108,130`, `claude_settings/mod.rs:81-91,367-376` | 1 |
| C5 | `prune_before` reads the whole file through `read_since_from(file, 0)` just to count drops. | `census_snapshot.rs:134` | 1 |
| C6 | `unreachable!` in non-test code without `#[expect(..., reason)]`. | `claude_settings/mod.rs:290,300` | 2a |
| C7 | `hooks::HookFile` and `claude_settings::Settings` both read and write `settings.json`; two layers over one file. | `hooks/mod.rs:52-132`, `claude_settings/mod.rs` | 2a |
| C8 | No `statusLine` setter; census edits it by hand through `serde_json::Value`. | census `setup.rs:164-178` | 2a |
| C9 | `SettingsError::Shape { key: String }` and `Changed` carry little a caller can match on. | `claude_settings/mod.rs:410` | 2a |
| C10 | `hooks/check.rs` holds `impl HookState`; the file name does not say what it owns. | `hooks/check.rs:9-33` | 1 |
| C11 | Signal handling and process-group teardown are duplicated in pons and vicarius (and acus `ctx` kills grandchildren on its own). No shared, feature-gated module with the one `unsafe` block. | pons `proxy/mod.rs:268-400`, vicarius `cmd_serve.rs:66-83`, acus-cli `cmd_ctx/mod.rs:380-394` | 2b |
| C12 | Possibly unused pub items: `format::Process`, `which::Platform`, `which::find_in`, `log::rotated`. `log::Writer` is used by pons, so it stays. | workspace grep, unverified | 1 |
| C13 | anyhow in public signatures (`cli::App`/`main`, `exit::finish`/`report`, `update::run`, `hint::find_hint`) contradicts the "libraries use thiserror" rule. | `cli/mod.rs:82-115`, `exit/mod.rs:39,47`, `update/mod.rs:243`, `hint/mod.rs:119` | 0 |
| C14 | `exit::Outcome::Code(u8)` sits next to `Found`/`Empty`; the exit-code space is untyped. | `exit/mod.rs:18-33` | later |
| C15 | `update::Tool` takes `&'static str` for name and version. | `update/mod.rs:36-39` | later |
| C16 | `UpdateError::status()` is `pub(super)`; tools cannot branch on HTTP status. | `update/error.rs:53` | later |

## Decisions

- C13: keep anyhow in the binary-facing part of core (`cli`, `exit`, `update::run`, `hint`) and
  write the exception into CLAUDE.md. Replacing it costs a lot and only satisfies the rule.
- C14 to C16 wait until a tool needs them; none does today.

## Not core material yet

Single-tool code stays where it is until a second tool needs it: acus truncation budget and ANSI
cleaning, census transcript paths and price tables, pons JSONC and JSON-lines framing.

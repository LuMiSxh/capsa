# pons agent instructions

Suite-wide rules are in ../AGENTS.md. This file covers pons only.

## Purpose

pons is a stdio proxy between Zed (the ACP client) and the Claude ACP adapter (`@agentclientprotocol/claude-agent-acp`). Zed starts it as a custom agent server. It forwards every message unchanged except for the rules documented in `README.md`: the cancel filter and steering, subagent folding (background cards, finished entries), the roster and notices, the task-notification filter, and transcript capability injection (asks the adapter for subagent text at `initialize`). `pons self doctor` reports how it is set up. Actions about pons itself are `pons self <action>`: `doctor`, `setup`, `update`, `uninstall`.

## Layout

Paths are relative to `pons/`. Neither pons nor velum uses another workspace crate today.

- `src/main.rs`: entry point; runs the proxy or a `pons self` action; `TOOL` names this binary for `capsa_core::update`.
- `src/cli.rs`: the flags and the `self` subcommand group with `doctor`, `setup`, `update` and `uninstall` (clap).
- `src/proxy/`: `mod.rs` starts the agent, wires the stdio and forwards signals; `session.rs` pumps one session's events through the engine and finishes it; `teardown.rs` tears the agent down (`teardown`); `io.rs` writes and reads the stdio lines.
- `src/engine/`: `route.rs` decides what happens to each message in both directions (`Engine::route_client`, `Engine::agent_request`) and calls the other rules; `mod.rs` holds `Config` and the engine state. Topic files hold the `impl Engine` parts: `initialize.rs`, `cancel.rs` (cancel filter), `steer.rs` (message send and steering), `response.rs` (prompt responses, turn ends), `updates.rs` (session updates, folding), `hold.rs` (held entries, agent exit), `clock.rs` (ticks, turn watchdog); `wire.rs` holds the message-shape helpers. `tests/` holds the unit tests grouped by topic and the ignored timing harness `bench.rs`; `tests/mod.rs` has the shared fixtures.
- `src/fold/`: subagent folding: `mod.rs` the `Folds` state and text clipping, `entry.rs` one tool-call entry, `background.rs` background cards and finish detection, `child.rs` child updates, `retire.rs` retired cards and the roster; `tests.rs` the unit tests.
- `src/notification.rs`: filter for task-notification and interrupt-marker messages.
- `src/roster.rs`: the subagent roster and plan messages.
- `src/permission.rs`: the opt-in `--auto-approve` policy for `session/request_permission`.
- `src/state.rs`: per-session state: pending cancels, open tools.
- `src/failsafe.rs`: `catch_unwind` around the rules, failure counting, and the switch to passthrough.
- `src/launcher/mod.rs` (tests in `tests.rs`): picks the agent command and builds the spawn plan; the program lookup (`PATH`, then the user tool directories) is `capsa_core::which`.
- `src/log/`: `mod.rs` the wire log for each start: writing, levels and its unit tests; `files.rs` the log directory and pruning; `render.rs` the text of each line (session updates are summarized).
- `src/doctor/`: `mod.rs` the `pons self doctor` report and the wire-log summary; `adapter.rs` the adapter check; `zed.rs` the Zed entry; `process.rs` the running pons processes; `render.rs` the text and JSON output; `tests.rs` the unit tests.
- `src/setup.rs`: `pons self setup`: reports the adapter or installs it through `capsa_core::pkg`.
- `tests/proxy/`: end-to-end tests. `main.rs` is the entry point, `harness.rs` holds `Harness`, which runs pons in front of `examples/mock_agent.rs`, a fake agent; the other files hold the tests by topic.

## Commands

- `cargo test -p pons`: unit tests and the end-to-end tests in `tests/proxy/`.
- `cargo test --profile release-unwind -p pons -- --ignored --nocapture engine_line_cost`: the engine timing harness, ns per line for client and agent lines.
- `cargo build --profile release-unwind -p pons`: the release build. Use this profile, not `release`: `src/failsafe.rs` catches panics in message rules with `catch_unwind`, and `release` sets `panic = "abort"`, which would turn a caught panic into a crash.
- `pons self doctor [--json|--human|--agent] [--log FILE]`: shows the launcher that would run, the Zed entry, running pons processes and a summary of the wire log. It never starts the adapter.
- `pons self update [--check|--force]`: replaces the binary with the newest `pons-v*` release (`capsa_core::update`; needs `GITHUB_TOKEN` or `gh auth login`). `--check` exits 0 when a newer release exists, 1 when current.
- `pons self uninstall [--dry-run]`: removes the binary only. Wire logs and the Zed entry stay.
- `pons self setup [--package-manager pnpm|bun|npm] [--dry-run] [--json|--human|--agent]`: reports the adapter and its version when found; otherwise installs `@agentclientprotocol/claude-agent-acp` globally. `--dry-run` installs nothing.

## Recipes

None yet.

## Contracts

- Every message is forwarded unchanged except for the documented rules. Lines pons cannot parse are forwarded raw.
- Permission requests are forwarded untouched unless the user opts in with `--auto-approve`; then pons answers requests for the matching tools only. `src/permission.rs` never auto-answers `exitplanmode` or `askuserquestion`. Elicitations are never touched.
- Folded text is plain ASCII. The test `everything_pons_emits_is_ascii` in `src/engine/tests/capability.rs` checks this.
- `clientCapabilities` is security-sensitive (root rule). The `initialize` arm of `Engine::route_client` forwards `clientCapabilities.fs` and `terminal` unchanged. Its only edit is adding `_meta["subagent-transcript"]` (`inject_transcript`). Changing that needs tests and a review.
- CLI flags, env vars and the Zed `agent_servers` entry are user configuration. `PONS_LOG` overrides the wire-log path; the root installers (`install.sh`, `install.ps1`) read `CAPSA_INSTALL_DIR` and `CAPSA_INSTALL_SCOPE`. Renaming one or changing a default breaks users; say so in the README Options and the CHANGELOG.
- Log directory per OS, when no `--log` is given: macOS `~/Library/Logs/pons`; Linux `$XDG_STATE_HOME/pons` or `~/.local/state/pons`; Windows `%LOCALAPPDATA%\pons\logs`. Each start writes `pons-<time>-<pid>.log` and keeps the newest few.
- `pons self doctor` reads log notes by their text (`summarize_log` in `src/doctor/mod.rs` matches `fail-safe`), so changing that wording means changing the parser too.

## Gotchas

- Shutdown order is in `teardown` in `src/proxy/teardown.rs`: close the agent's stdin and wait `--shutdown-grace-ms`, then SIGTERM the process group, then SIGKILL, then sweep the group. Each step logs an `exiting:` line; keep the order and the lines when changing it.
- When Zed advertises `clientCapabilities.subagents`, pons steps aside: folding, cards, roster, notices and the cancel filter are off. Use `--force-features` to exercise those paths.
- Zed may not inherit the shell `PATH`, so the launcher also searches user tool directories. Check the choice with `pons self doctor`, not by guessing.
- JSON key order follows serde_json `preserve_order`, which is workspace-wide.

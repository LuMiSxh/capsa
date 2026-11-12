# vicarius agent instructions

## Purpose

vicarius routes Claude Code subagents to DeepSeek while the DeepSeek balance lasts. This crate
holds the proxy (`serve`), the balance refresh, the derived agent files, the balance decision,
the settings and the launchd service.

## Layout

Paths are relative to `vicarius/`. vicarius depends on `capsa-core` only.

- `src/main.rs`: the clap CLI: `serve` and the `self` command group.
- `src/cmd_serve.rs`: `vicarius serve`; owns the tokio runtime, the key from `DEEPSEEK_API_KEY`,
  the shutdown signals and `spawn_balance_refresh`, the place where the refresh task starts.
- `src/gate.rs`: whether the DeepSeek route is open, the reason it is closed and the last balance.
  A close for `PaymentRequired` or `KeyRejected` bumps a generation; `Refresher::refresh` reads it
  before the fetch and opens through `open_if_unchanged`.
- `src/proxy/`: `mod.rs` (`ProxyConfig`, `Proxy::bind` and `run`), `handler.rs` (routing and
  relay), `tap.rs` and `usage.rs` (read tokens from the stream), `record.rs` (counters and
  `proxy.jsonl`), `headers.rs` (hop-by-hop and the log allowlist), `local.rs` (`/vicarius/*`).
- `tests/proxy/`: the proxy against mock upstreams on 127.0.0.1.
- `src/agents/`: derives `<from>-ds.md` from each configured agent file. `frontmatter.rs` does the
  line-based edit, `sync.rs` writes, refreshes and removes the files, treating a suspended agent as
  not configured, and `tests.rs` tests both.
- `src/balance.rs`: parses the `/user/balance` answer and decides availability; tested in
  `tests/balance.rs`.
- `src/refresh.rs`: `Refresher` fetches `/user/balance`, sets the gate, syncs the agent files and
  withdraws them on shutdown.
- `src/bulletin.rs`: publishes the gate to the capsa bulletin as `vicarius.json` from a blocking
  thread, and `withdraw` for uninstall. `tests/bulletin.rs` tests the withdraw; the publish is
  tested in `tests/refresh/`.
- `tests/refresh/`: the refresher against a mock balance endpoint, with a temporary agents dir.
- `src/service/`: the launchd plist, the `Launchctl` trait (faked in tests), the settings keys and
  the hook. `src/cmd_service/`: `self setup`, `start`, `stop` and the uninstall removal.
- `src/cmd_doctor.rs`, `src/cmd_hook.rs`, `src/cmd_status.rs`: the commands.
  `src/status_client.rs`: the plain-TCP client and lenient status types.
  `src/test_support.rs`: test doubles. `tests/service_cli.rs`: hook, status and doctor through the
  binary.
- `src/cmd_self.rs`: `self update` and `self uninstall` (feature `cmd-update`, on by default) and
  `self config`.
- `src/cmd_config.rs`: `self config` shows the settings; `set <key> <value>` changes a scalar;
  `agent add`, `remove`, `suspend` and `activate` edit the derived agents and sync them at once.
- `src/config.rs`: `config.toml` in the vicarius config directory, its defaults, its rules and the
  atomic save.
- `tests/cli.rs`: the binary, run with its config directory under a scratch home.

## Commands

- `cargo test -p vicarius`: all vicarius tests.
- `cargo run -q -p vicarius -- serve`: runs the proxy; `GET /vicarius/status` shows its state.
- `cargo run -q -p vicarius -- self config --help`: the options.

## Rules

Shared rules: see the root CLAUDE.md.

- A setting that breaks a rule is refused on `set`, on `agent add` and on load. A refused change
  writes nothing.
- `config.toml` is written through a temporary file in the same directory, then renamed over the
  old file.
- A change to a default or to a rule is user-visible: record it in CHANGELOG.
- The proxy carries the Claude OAuth token and the DeepSeek key. It binds `127.0.0.1` only, logs
  header names but values only for `anthropic-beta`, `anthropic-version` and `x-claude-code-*`,
  and keeps the key behind `DeepseekKey`, whose `Debug` is redacted. Keep a test for every new
  logged field.
- reqwest has no decompression features: bodies and `content-encoding` pass byte for byte. Do not
  add `gzip`, `brotli` or `system-proxy`.
- A closed gate answers 400, never 503: Claude Code adds "server-side issue" text to a 503.
- The settings `env` keys are written after the status answers and removed before the agent is
  booted out. A loaded agent whose plist changed is booted out and in again, because `kickstart`
  keeps the old definition.
- The hook never prints an error: every failure is silence and exit 0.
- The proxy answers 403 before routing to any request with an `Origin` header or a `Host` other
  than `127.0.0.1:<port>` or `localhost:<port>`. Every in-tree client sends the port in `Host`
  (`status_client` does). A change to this rule needs the tests in `tests/proxy/origin.rs`.

## Gotchas

- macOS takes the config directory from `$HOME/Library/Application Support`, not from
  `XDG_CONFIG_HOME`; the tests set both.
- `agent add` does not read `~/.claude/agents/<from>.md`; it checks the name and the model only,
  then syncs and reports a missing source. A model outside `deepseek-flash` and `deepseek-v4-pro`
  is saved with a warning, so a new upstream id keeps working.
- `set port` saves `config.toml` before the restart, because the agent reads it at boot; a port
  that does not answer rolls the config, the agent and the settings back to the old port. Every
  other setting is re-read on each sync.
- `start` and `setup` refuse a hand-set `ANTHROPIC_BASE_URL` or `CLAUDE_CODE_MAX_CONTEXT_TOKENS`
  that differs from the config, before anything is written. `Install::launchd()` is `None` off
  macOS, so no launchctl or `id -u` runs there.
- A derived file is owned only when its frontmatter carries the `# Generated by vicarius from …`
  marker. Without it, the file is a conflict and is never written or removed.

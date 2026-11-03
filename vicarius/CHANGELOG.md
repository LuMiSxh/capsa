# Changelog

## [Unreleased]

### Added

- `vicarius serve` runs the local proxy in the foreground on `127.0.0.1:<port>` until SIGTERM or
  SIGINT. It routes each `/v1/messages` and `/v1/messages/count_tokens` request by the body's
  `model`: `deepseek-*` goes to DeepSeek's Anthropic API with `DEEPSEEK_API_KEY`, everything else
  to `api.anthropic.com` unchanged. Responses stream without buffering. A closed gate, a DeepSeek
  402 or a DeepSeek 401 answers HTTP 400 with `x-should-retry: false` and no upstream call. Each
  request is logged to `proxy.jsonl` in the state directory (never a secret value), and
  `GET /vicarius/status` shows the gate and the counters.
- `vicarius serve` reads the DeepSeek balance at start and every `balance_refresh_minutes` and
  opens or closes the DeepSeek route with it. A 401 or 402 from DeepSeek closes it, a later
  successful refresh reopens it, and a failed refresh keeps the gate as it was.
- `vicarius serve` keeps the derived `<agent>-ds.md` files in `$CLAUDE_CONFIG_DIR/agents` (else
  `~/.claude/agents`) in step with the gate: it re-reads `config.json` on every sync, syncs every
  60 s, removes a derived file whose source is gone, and removes all of them on SIGTERM or SIGINT.
- `POST /vicarius/refresh` runs a refresh and a sync at once. `GET /vicarius/status` adds `agents`
  (the last sync report) and `refresh` (`last_ok_at`, `last_error`, `next_at`).
- Each configured agent gets `~/.claude/agents/<from>-ds.md`, derived from its file with `name`,
  `model` and `description` changed and the rest left byte for byte. A file without the vicarius
  marker is never overwritten or removed.
- The DeepSeek balance answer is parsed and availability is decided against the USD minimum. An
  account with only CNY is available while its balance is above zero.
- `vicarius self setup`, `start`, `stop` and `doctor` manage the launchd agent
  `io.github.lumisxh.vicarius` (macOS only). It starts through `zsh -c`, so `DEEPSEEK_API_KEY`
  comes from the login environment and never reaches the plist. The settings `env` keys are
  written only after the service's status answers and removed before the agent is booted out.
- `vicarius status` (agent text, `--human`, `--json` passes the service's bytes) and the
  SessionStart hook `vicarius hook session-start`, which prints the DeepSeek agent line only when
  the gate is open and an agent file is ready. Under 15 ms with the service up or down (debug
  build, `/usr/bin/time`, 3 runs).
- `vicarius self uninstall` stops the service and removes the agent, the plist and the
  SessionStart hook before it removes the binary.
- `self config set port` restarts a loaded agent on the new port and rewrites
  `ANTHROPIC_BASE_URL` once its status answers there; a port that does not answer leaves the
  settings unchanged. `self config set max-context` rewrites `CLAUDE_CODE_MAX_CONTEXT_TOKENS` when
  it holds the value vicarius wrote; a hand-set value is kept.
- Installable with `install.sh` and `install.ps1`, released by the release workflow as
  `vicarius-vX.Y.Z`.
- The proxy answers 403 to any request that carries an `Origin` header or whose `Host` is not
  `127.0.0.1:<port>` or `localhost:<port>`, on every path including `/vicarius/*`, so a web page
  cannot spend the DeepSeek key or trigger a refresh. Refusals are logged and counted in
  `counters.forbidden`.
- A balance refresh that started before a DeepSeek 402 or 401 no longer reopens the route after
  the close.
- `self config set port` rolls back when the new port does not answer: `config.json`, the loaded
  agent and the settings return to the old port.
- `self start` and `self setup` refuse a hand-set `ANTHROPIC_BASE_URL` or
  `CLAUDE_CODE_MAX_CONTEXT_TOKENS` that differs from the config, before writing or launching.
- `self uninstall` off macOS runs neither `id -u` nor launchctl; it removes only the SessionStart
  hook and the matching env keys.
- A restart waits up to 5 s for launchd to unload the agent before bootstrapping it.
- Initial crate with `vicarius self config`: shows the settings, sets the port, the minimum
  balance, the refresh interval and the context window, and adds or removes the agents that get a
  DeepSeek variant. `self update` and `self uninstall` work as in the other tools.

# Plan: DeepSeek subagents (`vicarius`) and a provider-neutral census

Status: draft, 2026-11-03. Nothing here is built yet.

## Goal

One harness: Claude Code on the Claude Pro subscription. While DeepSeek credit lasts, the main
agent may delegate to `senior-worker-ds`, a variant of `senior-worker` on DeepSeek V4.1 Flash
(`deepseek-flash`) billed to the DeepSeek API key. The main agent still starts with `worker`
(Haiku); it asks Luca before using a `-ds` agent and reports what it cost.

## Evidence from the spike

Throwaway Python proxy, Claude Code 2.1.309, main agent on Haiku via subscription OAuth:

- With only `ANTHROPIC_BASE_URL` set, the subscription login stays active; the proxy forwards
  `Authorization` and `anthropic-beta` (it carries `oauth-2025-04-20`) unchanged, all 200.
- A subagent with `model: deepseek-flash` in its frontmatter sends that id verbatim. The proxy
  routes on it to `https://api.deepseek.com/anthropic/v1/messages` with the DeepSeek key.
- An 8-request tool loop, two parallel DeepSeek subagents, and a mixed DeepSeek/Claude turn all
  passed with **no body normalization**. DeepSeek accepted `thinking: adaptive` and
  `output_config.effort`.
- `x-claude-code-agent-type` is `custom` for every user agent, so routing uses the model id.
- Claude Code prints `[claude-code:unrecognized_model] {...}` to stderr. The model never sees it;
  it shows in `claude -p` stderr and in the ACP adapter's log.
- Not yet tested: `deepseek-v4-pro`, compaction of a long DeepSeek subagent, long runs.
- Second round:
    - The agent list is fixed at session start: an agent file written by a SessionStart hook is
      not usable in that session, with or without an existing `agents/` directory.
    - A proxy answer of HTTP 400 with `x-should-retry: false` stops the subagent after one
      request; the spawning agent gets `Agent terminated early due to an API error: API Error:
400 <message>` and asks how to go on. A 503 also does not retry but appends Claude Code's
      own "server-side issue" text, so 400 is the status to use.
    - Gateway model discovery is skipped under the subscription login (it needs an API
      credential), keeps only ids containing `claude`/`anthropic`, and ignores
      `max_input_tokens`. It is not used.
    - The Zed adapter (`@agentclientprotocol/claude-agent-acp` 0.85.0) runs the Agent SDK with
      `settingSources: ["user","project","local"]`, so the `env` block of
      `~/.claude/settings.json` applies. It overrides `ANTHROPIC_BASE_URL` only when the ACP
      client sets a provider (`providers/set`).
- Balance endpoint `GET https://api.deepseek.com/user/balance` works with `Authorization: Bearer`.

## Part 1: `vicarius` (new tool)

### CLI

| Command                              | Does                                                                    |
| ------------------------------------ | ----------------------------------------------------------------------- |
| `vicarius serve`                     | Runs the proxy in the foreground (launchd calls this).                  |
| `vicarius status`                    | Proxy state, DeepSeek balance, spend per session and day.               |
| `vicarius hook session-start`        | SessionStart hook: syncs the agent file, prints the context line.       |
| `vicarius self setup`                | Installs the launchd agent, the hook and the settings `env`; starts it. |
| `vicarius self start` / `stop`       | Starts or stops the service and switches `ANTHROPIC_BASE_URL` with it.  |
| `vicarius self config`               | Shows and edits the configuration, same shape as `census self config`.  |
| `vicarius self doctor`               | Checks service, port, settings, key, balance, agent file, hook.         |
| `vicarius self update` / `uninstall` | As in every capsa tool.                                                 |

`start`/`stop` sit under `self` on Luca's request; they are part of installation health here
because the proxy is the session's network path. Output follows the capsa contract (`render`,
`--human`, `--json`); `hook session-start` is protocol output.

### Proxy

- Rust, tokio + hyper 1.x, rustls; tokio stays inside this crate.
- Binds `127.0.0.1` on the configured port, default `3337` ("DEEP" on a phone keypad). Any
  `deepseek-` model routes the same way, so `deepseek-flash` and `deepseek-v4-pro` both work.
- Routes on the request body's `model`: prefix `deepseek-` goes to DeepSeek, everything else to
  `api.anthropic.com` byte for byte. Only the body of `/v1/messages` and
  `/v1/messages/count_tokens` is parsed; other paths are passed through untouched.
- DeepSeek route: drops `Authorization`, `x-api-key` and `cookie`, sets `x-api-key` from
  `DEEPSEEK_API_KEY`. Normalization rules (drop `output_config`, `context_management`, beta tool
  fields; thinking handling) exist but are off by default, switched on per rule in the config if
  DeepSeek starts rejecting fields.
- Streams responses unbuffered and forwards `retry-after`, `x-should-retry`,
  `anthropic-ratelimit-*` and error bodies unchanged.
- Reads `usage` from the SSE `message_start`/`message_delta` events as they pass (no buffering)
  and records tokens per request for `status` and census.
- Never logs secret header values; logs header names, route, model, status, tokens.
- Log: `~/.local/state/vicarius/proxy.jsonl` via `capsa_core::log`.

### Availability

- The service refreshes the balance every 10 minutes and on demand.
- Claude Code reads the agent list once at session start, so the **service** keeps
  the derived `~/.claude/agents/<from>-ds.md` files in step with the balance on every refresh:
  written at >= $1,
  removed below $1, when the key is missing, and by `self stop`/`uninstall`. A new session sees
  the state of the last refresh (at most 10 minutes old).
- The SessionStart hook only prints the context line, and only when the service is up, the
  balance is >= $1 and the agent file exists. It reads the service's cached state over
  localhost, so session start does no outbound request.
- **During a run, balance < $1 or DeepSeek answers 402:** the proxy answers the DeepSeek request
  with HTTP 400, `x-should-retry: false`, body
  `{"type":"error","error":{"type":"invalid_request_error","message":"DeepSeek is currently
unavailable (balance below $1). Use worker or senior-worker."}}`. The spawning agent receives
  it as the subagent's failure (verified in the spike). This also covers sessions that started
  while the file existed.
- The agent file lives in `~/.claude/agents/` (user level, no repository changes).

### Configuration

`vicarius self config`, same mechanics as census (`census/src/cmd_config.rs`); shared code moves
to `core`. Keys: `port = 3337`, `min_balance_usd = 1.0`, `balance_refresh_minutes = 10`,
`max_context_tokens = 400000` (written as `CLAUDE_CODE_MAX_CONTEXT_TOKENS` into the settings
`env`; one value for all DeepSeek models, since Claude Code applies it to every unrecognized id),
and the derived agents, default `[{ "from": "senior-worker", "model": "deepseek-flash" }]` (later
e.g. `{ "from": "consultant", "model": "deepseek-v4-pro" }`). The file is `config.json` in
`capsa_core::paths::config_dir`, like census; `self config` shows it, `self config set <key>
<value>` changes a scalar, `self config agent add <from> --model <id>` / `agent remove <from>`
edit the list. Changing `port` or `max_context_tokens` rewrites the settings `env`; running
sessions need a restart, and the command says so.

### Derived DeepSeek agents

Each `[[agent]]` entry derives `<from>-ds` from `~/.claude/agents/<from>.md`:

- Line-based edit of top-level frontmatter keys only (no YAML dependency), rest byte for byte:
  `name` becomes `<from>-ds`; `model` is replaced or added; `description` gets
  `DeepSeek variant of <from> on <model>, billed to the DeepSeek API credit.` appended.
- Written to `~/.claude/agents/<from>-ds.md` with a frontmatter comment marking it generated.
- Synced on every balance refresh and when the source mtime changes.
- vicarius deletes only files it wrote (recorded in its state); hand-written `*-ds.md` files and
  unparsable sources are `doctor` findings.

### Hook context

- Context line when available, listing every derived agent, for example:
  `vicarius: DeepSeek agents available (balance $18.40): senior-worker-ds (deepseek-flash,
  variant of senior-worker). Start with the Claude agents; ask Luca before using a -ds agent and
  report its cost afterwards (vicarius status). Do not pass a model parameter when spawning one.`
- The output style in MacSet (`agent/claude/output-styles/luca.md`) gets one rule: `-ds` agents
  only when the vicarius context line is present, only after asking.

### Compaction at 400k

Claude Code assumes 200k for an unknown model id. Per the model-config docs ("Which IDs
`CLAUDE_CODE_MAX_CONTEXT_TOKENS` applies to"), that variable applies directly to an unrecognized
id that does not start with `claude-` and carries no `[1m]`; for recognized Claude models it
takes effect only with an extra opt-in variable. So `self start` sets
`CLAUDE_CODE_MAX_CONTEXT_TOKENS=400000` in the settings `env`, which moves `deepseek-flash` to
400k and leaves Claude models untouched. The docs say the startup warning disappears once the
window is declared; whether the stderr `unrecognized_model` line also goes is to be checked.

### Service lifecycle

- macOS: launchd user agent `~/Library/LaunchAgents/<label>.plist` with `RunAtLoad` and
  `KeepAlive`. Linux (systemd user unit) and Windows later; `self setup` refuses with a hint
  there until then.
- `self start` bootstraps the agent and adds `ANTHROPIC_BASE_URL` and
  `CLAUDE_CODE_MAX_CONTEXT_TOKENS` to the `env` block of `~/.claude/settings.json` via
  `capsa_core::claude_settings` (which gains env-key support next to its hook support).
  `self stop` removes them, then boots the agent out. Running sessions keep their environment;
  `stop` says so and names the restart.
- If the proxy dies while sessions point at it, every request of those sessions fails. KeepAlive
  restarts it; `doctor` reports it.
- Side effects of a custom base URL to accept: Remote Control is disabled, fast mode bypasses
  the proxy.

### Zed

The adapter applies the settings `env` (see the spike), so pons needs no change. If Zed starts
sending `providers/set`, the adapter's own base URL wins and DeepSeek routing stops in Zed;
`self doctor` cannot see that, so it is a known limit.

## Part 2: census, provider-neutral

### Usage by provider and agent

- Each priced row carries a provider (`anthropic`, `deepseek`, ...) taken from the model id via
  the price source, and the agent (main, subagent type) it came from.
- Subscription limits (`limits/`, the ledger digest, calibration, forecasts) count only rows
  billed to the subscription. DeepSeek rows never move the 5h or week forecast.
- `census usage` gets a provider breakdown and shows API spend separately from subscription
  usage. Codex keeps working through the same path.

### Prices from a source, not from code

- Source: LiteLLM's `model_prices_and_context_window.json` (per-token input, output, cache read,
  cache write, long-context tiers, `max_input_tokens`, provider). Verify that it lists
  `deepseek-flash` and the Claude 5.x ids with current prices.
- Cache: `~/.cache/census/prices.json`, refreshed when older than 7 days (configurable via
  `census self config`). The refresh runs in `census usage`/`limits`, never in `census hook` or
  `statusline`, which stay offline and fast.
- No cache and no network: cost is shown as unknown, not guessed. A local override file in the
  census config takes precedence (for DeepSeek's peak/off-peak prices, which the source may not
  model).
- `PRICES` and `LONG_TIERS` in `census/src/usage/prices.rs` and the `WEIGHTS` table go away or
  become derived from the source.
- The HTTP client comes from what `capsa_core::update` already uses; no new dependency.

## Part 3: other repositories

- MacSet: output style rule (above). MacSet does not copy or delete `*-ds.md`; vicarius owns
  them.
- pons: no change.

## Verify before building

1. Done: `CLAUDE_CODE_MAX_CONTEXT_TOKENS=400000` gives `deepseek-flash` 400000 and leaves
   `claude-haiku-5-5` at 1000000, also side by side in one session. The stderr
   `unrecognized_model` line stays; it never reaches the model, so it is accepted.
2. Done: LiteLLM's file lists `deepseek-flash`, `deepseek-v4-pro` and all Claude 5.x ids with
   input, output, cache read/write (5 min and 1 h) and `*_above_100k_tokens` tiers. Its DeepSeek
   prices are the peak prices.
3. Compaction of a long `-ds` agent run on DeepSeek.

## Order of work

1. Check item 1 with one cheap run; items 2 and 3 during the build.
2. `vicarius`: crate skeleton, proxy, balance service, agent-file sync, `serve`/`status`.
3. `vicarius self` (setup, start, stop, doctor) and `hook session-start`.
4. census: price source and cache.
5. census: provider and agent breakdown, subscription-only limits.
6. MacSet output style.
7. Review (`reviewer`; the proxy's credential handling gets a security review), then commits per
   area.

<div align="center">
<img src="../.github/assets/vicarius.svg" width="112" height="112" alt="vicarius logo" />

# vicarius

**Routes Claude Code subagents to DeepSeek while credit lasts.** Claude traffic passes through unchanged on the subscription login; `deepseek-*` models go to DeepSeek's Anthropic-compatible API.

[![Rust 1.99+](https://img.shields.io/badge/Rust-1.99%2B-a6d935?style=flat-square&labelColor=23252a)](https://www.rust-lang.org/)
![Platform](https://img.shields.io/badge/platform-macOS-a6d935?style=flat-square&labelColor=23252a)
[![MPL-2.0 license](https://img.shields.io/badge/license-MPL--2.0-a6d935?style=flat-square&labelColor=23252a)](../LICENSE)
[![Version](https://img.shields.io/badge/version-0.1.0-a6d935?style=flat-square&labelColor=23252a)](CHANGELOG.md)

**[capsa](../README.md)** · [Changelog](CHANGELOG.md)

</div>

vicarius is one of the [tools in capsa](../README.md#tools). It runs a local proxy on `127.0.0.1:3337` that Claude Code uses as its `ANTHROPIC_BASE_URL`.

## Install

vicarius runs as a launchd service, so it is macOS only for now. Install the release build with the capsa installer, then set up the service:

```sh
./install.sh vicarius         # macOS
vicarius self setup           # launchd service, hook and settings
```

Store the DeepSeek key in `~/.config/secrets.env`:

```sh
export DEEPSEEK_API_KEY=...
```

The launchd service picks the key up through zsh's `.zshenv`. vicarius never writes the key to disk.

Build from source, with the toolchain pinned in `rust-toolchain.toml` (see [Build from source](../README.md#build-from-source)): `cargo install --locked --path vicarius`.

## Commands

`vicarius <command>` prints its result to stdout. The proxy itself runs under `vicarius serve`; the other commands talk to it or to the settings.

| Command                              | What it does                                                                                                     |
| ------------------------------------ | ---------------------------------------------------------------------------------------------------------------- |
| `vicarius serve`                     | Runs the proxy in the foreground until SIGTERM or SIGINT; launchd starts it                                      |
| `vicarius status`                    | The gate, the balance, the last refresh, the requests, the tokens and the agents                                 |
| `vicarius hook session-start`        | SessionStart hook: prints the line that lists the DeepSeek agents when they are available                       |
| `vicarius self setup`                | Installs the launchd service, the hook and the settings `env`, then starts the service                          |
| `vicarius self start` / `stop`       | Starts or stops the service and sets or removes `ANTHROPIC_BASE_URL` in the settings with it                    |
| `vicarius self config`               | Shows the settings and changes them; see Settings                                                               |
| `vicarius self doctor`               | Checks the service, the port, the settings, the key, the balance, the agent files and the hook                  |
| `vicarius self update` / `uninstall` | As in the other tools                                                                                           |

Output is agent text by default. `--human` adds colour on a terminal, `--json` prints the report as JSON, and `--format agent|human|json` is the same choice. `NO_COLOR` turns colour off. Exit codes are 0 for results, 1 for nothing found and 2 for an error.

## Settings

`vicarius self config` reads `config.json` in the vicarius config directory. `vicarius self config set <key> <value>` changes one setting:

| Key               | Default  | What it does                                                                                                       |
| ----------------- | -------- | ------------------------------------------------------------------------------------------------------------------ |
| `port`            | `3337`   | Local port of the proxy. A change rewrites the settings `env`; running sessions need a restart                     |
| `min-balance`     | `1.0`    | DeepSeek balance in USD below which the `-ds` agents are withdrawn                                                 |
| `balance-refresh` | `10`     | Minutes between two balance checks                                                                                 |
| `max-context`     | `400000` | Context window in tokens that Claude Code assumes for the DeepSeek models, as `CLAUDE_CODE_MAX_CONTEXT_TOKENS`      |

`vicarius self config agent add <from> --model <id>` adds a `<from>-ds` variant of `~/.claude/agents/<from>.md` that runs on the model `<id>`, and `agent remove <from>` removes it. The default is one agent, `senior-worker` on `deepseek-flash`, which gives `senior-worker-ds`.

## Subagents

Each configured agent gets `~/.claude/agents/<from>-ds.md`: the source file with `name`, `model` and `description` changed and the rest left byte for byte. The service writes it while the balance is at least `min-balance`, and removes it when the balance drops, when the key is missing and when the service stops. A file without the vicarius marker is never written or removed.

Claude Code reads the agent list once, at session start, so a new session sees the state of the last refresh. The SessionStart hook adds a line to the main agent's context that lists the available `-ds` agents and their models.

## Balance

The service refreshes the balance every `balance-refresh` minutes. When it drops below `min-balance`, the `-ds` agents are withdrawn at the next refresh. A subagent that is already running gets HTTP 400 `DeepSeek is currently unavailable`, and the agent that spawned it sees that message.

## Limits

- Anthropic does not support routing non-Claude models through a gateway; see the [LLM gateway docs](https://code.claude.com/docs/en/llm-gateway).
- Remote Control is disabled while the base URL points at the proxy.
- Fast mode bypasses the proxy.
- A running session needs a restart after `vicarius self start` or `stop`.
- Claude Code prints a `[claude-code:unrecognized_model]` line to stderr for DeepSeek models. The line is harmless: the model never sees it.

## Security

- The proxy binds `127.0.0.1` only.
- It never logs secret header values. `proxy.jsonl` in the state directory records the route, model, status, tokens and header names of each request.
- vicarius never writes the DeepSeek key to disk; it only reads `DEEPSEEK_API_KEY`.

## Development

```sh
cargo fmt --all --check
cargo clippy -p vicarius --all-targets --all-features --locked -- -D warnings
cargo test -p vicarius
```

For the layout and conventions, see [AGENTS.md](AGENTS.md). Release notes go in [CHANGELOG.md](CHANGELOG.md); the release process is under [Releases](../README.md#releases) in the capsa README.

## License

vicarius is licensed under the [Mozilla Public License 2.0](../LICENSE).

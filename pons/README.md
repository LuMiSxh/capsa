<div align="center">
<img src="../.github/assets/pons.svg" width="112" height="112" alt="pons logo" />

# pons

**Transparent stdio proxy between Zed and the Claude ACP adapter.** pons forwards every message unchanged, except for the subagent problems of Zed and the Claude adapter that it works around, and writes a wire log of the traffic.

[![Rust 1.99+](https://img.shields.io/badge/Rust-1.99%2B-d9764e?style=flat-square&labelColor=23252a)](https://www.rust-lang.org/)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-d9764e?style=flat-square&labelColor=23252a)
[![MPL-2.0 license](https://img.shields.io/badge/license-MPL--2.0-d9764e?style=flat-square&labelColor=23252a)](../LICENSE)
[![Version](https://img.shields.io/badge/version-0.9.0-d9764e?style=flat-square&labelColor=23252a)](CHANGELOG.md)

**[capsa](../README.md)** · [Changelog](CHANGELOG.md)

</div>

pons is one of the [tools in capsa](../README.md#tools). It sits between Zed (the ACP client) and the Claude ACP adapter (`@agentclientprotocol/claude-agent-acp`) and forwards every message unchanged, except for the cancel filter and steering, subagent folding and background subagents, task-notification and interrupt-marker filtering, and a few adapter workarounds.

## Install

pons is distributed from [LuMiSxh/capsa](https://github.com/LuMiSxh/capsa). See [Install](../README.md#install) in the capsa README for the installer options.

macOS and Linux, pons only:

```sh
curl -fsSL https://raw.githubusercontent.com/LuMiSxh/capsa/main/install.sh | sh -s -- pons
```

Windows (PowerShell), pons only:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/LuMiSxh/capsa/main/install.ps1))) pons
```

The installers put `pons` in `~/.local/bin` (`CAPSA_INSTALL_DIR` changes that) after checking the release against its `.sha256` file. Then run `pons self setup` to make sure the Claude ACP adapter is installed, and point Zed at pons as described in [Zed](#zed).

Build from source, with the toolchain pinned in `rust-toolchain.toml` (see [Build from source](../README.md#build-from-source)). The `release-unwind` profile keeps panics unwinding, which the fail-safe needs:

```sh
cargo install --path pons --profile release-unwind
```

This puts `pons` in `~/.cargo/bin`. The former repository, `LuMiSxh/pons`, is frozen; build from capsa.

## Commands

pons is started by Zed, not by hand (see [Zed](#zed)).

| Command                               | What it does                                                                                                            |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `pons [OPTIONS] [-- AGENT ARGS...]`   | The proxy; the flags are listed under [Options](#options)                                                               |
| `pons self doctor`                    | Report on the install, the launcher, Zed's settings, the running processes and the wire log; exits 1 when a check fails |
| `pons self setup`                     | Makes sure the Claude ACP adapter is installed; see [self setup](#pons-self-setup)                                      |
| `pons self update [--check\|--force]` | Replaces the binary with the latest release                                                                             |
| `pons self uninstall [--dry-run]`     | Removes the binary                                                                                                      |

The `self` commands take the output flags: `--agent` (the plain default), `--human` (colour on a terminal) and `--json` (one JSON object). `NO_COLOR` turns colour off. Exit codes are 0 for success, 1 when `pons self doctor` finds a failed check, and 2 for an error; `pons self setup` exits 2 when no package manager is found.

## Zed

With `pons` on `PATH` (the installers above), add this to `settings.json`. The minimal form needs neither `--` nor the agent path:

```json
"agent_servers": {
  "claude-pons": {
    "type": "custom",
    "command": "pons",
    "args": [],
    "env": {}
  }
}
```

Each start writes its own wire log (see [Wire log](#wire-log)). To write one fixed file instead, use `"args": ["--log", "/tmp/pons.log"]`.

Zed may not inherit your shell `PATH` when launched from the Dock or Start menu. If it cannot find `pons`, set `command` to the absolute path instead, such as `/Users/you/.cargo/bin/pons` or `/Users/you/.local/bin/pons` (Zed does not expand `~`).

### Agent command

Without `-- <agent> [args...]` pons picks the first of these it can find:

1. `claude-agent-acp`
2. `npx -y @agentclientprotocol/claude-agent-acp`
3. `pnpm dlx @agentclientprotocol/claude-agent-acp` (or `pnpx @agentclientprotocol/claude-agent-acp`)
4. `bunx @agentclientprotocol/claude-agent-acp`

Each program is searched on `PATH` first, then in the user directories that Zed's `PATH` often lacks: `~/.local/bin`, `~/.cargo/bin`, the pnpm and Bun bin directories, `/opt/homebrew/bin`, `/usr/local/bin`, Volta, fnm's default node and every nvm node (newest first). On Windows the candidates are `%APPDATA%\npm`, `%LOCALAPPDATA%\pnpm`, `%USERPROFILE%\.bun\bin`, `%LOCALAPPDATA%\Volta\bin` and `%USERPROFILE%\.cargo\bin`, with `PATHEXT` extensions tried (`npx.cmd`, `bunx.exe`; a bare `npx` script is not runnable there). When pons locates a program itself, the agent's `PATH` gets that program's directory and the existing user directories appended, so a `#!/usr/bin/env node` shim finds `node`. An explicit command after `--` always wins, and a bare name is resolved the same way.

On Windows, `.cmd` and `.bat` shims are spawned through `cmd.exe /c` by Rust's `Command`, which escapes their arguments and refuses what it cannot make safe. pons also rejects arguments containing `"`, `%`, `^`, `&`, `|`, `<`, `>`, `!`, a backtick or a line break for such a shim. `pons self doctor` shows what would be chosen.

The environment (including Anthropic auth) is inherited by the agent process.

### MCP servers for the session

`--mcp-server NAME=COMMAND[,ARG...]` (repeatable) adds a stdio MCP server to every `session/new`, `session/load` and `session/resume` request that Zed sends through pons, so the server exists only in those sessions. A server whose name Zed already sends keeps Zed's entry.

## Platforms

Release binaries: macOS (arm64, x86_64), Linux (x86_64, arm64) and Windows (x86_64). macOS is the primary platform; CI builds and tests on all three. On Unix the agent runs in its own process group, and SIGTERM, SIGHUP and SIGINT are forwarded to the group, which is killed if it lingers for 5 seconds. On Windows there are no process groups or SIGTERM: after the shutdown grace period (or on Ctrl+C) the agent is ended with `taskkill /T /F`, which also takes its child processes. A Job object would be tighter; this path is untested by the maintainer.

## Cancel and steering

Zed sends `session/cancel` before each new `session/prompt` while a prompt is unresolved (about 100 ms apart). The adapter keeps a turn open while background subagents run, and an interrupt kills them. So when a prompt is outstanding and at least one subagent is in flight, pons does not forward the cancel at once, whether or not the main agent is streaming. It answers the pending prompt itself with `stopReason: "cancelled"` (the agent's later response to it is swallowed, so Zed can go on) and holds the cancel for `--cancel-defer-ms` (default 400):

- **A message in the window** (a `session/prompt` arrives before it ends): the cancel is dropped for good and the subagents live on. Their running cards and the subagent plan are sent to Zed again, since Zed had marked them stopped with the cancelled response. The message is delivered by `--interrupt-mode`:
    - `steer` (default if the adapter advertised `_meta.steering.supported` at `initialize`) sends the adapter's `_session/steering {sessionId, prompt}` request instead of the prompt, so the text is injected into the running turn.
    - `queue` (the default without steering, and the fallback when steering fails or is declined) forwards the prompt unchanged and the adapter queues it behind the running turn.
    - `cancel` is the old behaviour: the cancel is forwarded, which kills the subagents.
- **No message in the window** is a real Stop: the subagent cards are marked stopped and the cancel is forwarded to the agent.
- **Stop escape:** a second `session/cancel` within `--stop-window-ms` (default 3000) of a held one is forwarded at once. `--stop-window-ms 0` disables the escape, so the held cancel waits for its window.

With steering, Zed's new prompt id is not sent to the agent. The real turn runs under the old prompt id (already answered as cancelled), so pons keeps the new id open and, when the agent answers the old prompt, answers the new one with that same result. Updates stream meanwhile, and the background cards of the old turn finish with the new prompt's response. The steering request carries `_meta.steering.idleBehavior = "promptRequired"`, so an idle adapter declines instead of starting a turn nobody answers.

Without subagents a cancel is forwarded at once, as before. `--quiet-ms` is deprecated and ignored. `--steer-hold-ms` and the other adapter workarounds are under [Adapter workarounds](#adapter-workarounds).

## Subagents

Subagent `session/update`s (those carrying `_meta.claudeCode.parentToolUseId`) are folded into the parent Task/Agent tool call as a throttled `tool_call_update` with a compact text log (`running: Read src/a.rs`, `done: ...`, `failed: ...`, `agent: last agent text`, last 30 lines). pons only emits plain ASCII.

Subagent text is not sent by the adapter unless asked. Zed advertises neither the `subagent-transcript` capability nor native subagents, so without help pons sees only tool calls. pons adds `clientCapabilities._meta["subagent-transcript"] = true` to the `initialize` it forwards (creating `clientCapabilities` and `_meta` if absent, and logging one note). It skips this when it steps aside, when the client already set the key, or with `--no-fold` or `--no-subagent-text`. The subagent's text is then folded like its tool calls and never forwarded flat.

Permission requests are never touched unless you opt in with `--auto-approve` (see [Adapter workarounds](#adapter-workarounds)). Then pons answers the requests of the matching tools itself. Elicitations are never touched. Any tool call that a child names in `parentToolUseId` is a parent, whatever its name.

### Background subagents

The adapter marks an `Agent` call completed the moment it launches a background subagent ("Async agent launched successfully") and sends no completion event to clients without native subagent support, which includes Zed. pons rewrites that update to `in_progress` (`running in the background`), keeps folding the subagent's activity into the card, and finishes it with a last log line and status `completed`: `done - N tool calls`. The signals it can observe are, in order of strength:

- **Prompt response (confirmed).** The adapter holds a `session/prompt` open until the subagents that turn spawned are gone, so a response to a prompt that is still the newest one finishes the background cards that prompt launched. A response that only arrives because a newer prompt took over (a handoff) finishes nothing; the cards then belong to the newer one.
- **Task notification (confirmed).** The adapter's `task-notification` usage update is the only thing it puts on the wire when one background subagent ends, and it does not say which. pons attributes it to a card with no open tool call, preferring one whose last event was its own text, then the one quiet the longest (`--notify-settle-ms`, default 0, can delay the decision). With no such card it finishes nothing and only logs that.
- **Silence (provisional).** A card whose last event was the subagent's own text, with no tool call open, and no activity for `--subagent-idle-ms` (default 20000, 0 disables) is finished as `done (no activity for 20s) - N tool calls`. A card whose last event was a tool result or thinking, or that has a tool call open, is never finished this way, because a subagent thinks or runs a long tool silently. Its title and roster line show `quiet 1m 5s` instead. With `--no-subagent-text` pons cannot see text, so only `--subagent-max-quiet-ms` (default 600000, 0 disables) ends a silent card. Silent finishes are provisional: no notice, the cancel filter keeps protecting the card, and later activity reopens it as a background card with its log.
- **Stop.** A real Stop marks the running cards `failed` (`stopped - N tool calls`).

A `Monitor` call keeps its subagent open while the monitor may still stream events: until its timeout (`timeout_ms` in the call's input; 30 min when there is none) plus 30 s. Silence does not finish such a card in that time. Only a Stop, the response to the prompt that launched it, or a `task-notification` that no other background card qualifies for ends it earlier.

Running background subagents also hold the cancel filter across turns.

A background card's `title` carries live counters: `<title>: running 15s, 3 tool calls, last: Read src/a.rs` (elapsed time measured by pons from the launch, refreshed at most every 5 s, the rest on the fold throttle). At the end it reads `<title>: done, 3 tool calls, 42s`, or `stopped` after a Stop.

### Finished entry

When pons finishes a card it also emits a new tool call (`pons-finished-<parent>`, one stable id per card, kind `other`, title `<title> finished`, or `<title> stopped` with status `failed` on Stop). If the card turns out to be alive after the entry was sent, the entry is updated (`in_progress`, `resumed`) and, on the next finish, completed with the result. A second entry is never appended. Its text is the tool call count, then the subagent's final message (its last contiguous agent text, at most 2000 characters, the head kept and cut with `...`), so Zed shows the result at the end of the thread:

```
Review the diff finished
3 tool calls
Found 2 issues.
1. src/a.rs:10 unused import
2. src/b.rs:4 missing test
```

The finished card shows the same message as its last `agent:` line. A running card shows the latest 300 characters of the current text.

The entry never splits the main agent's text. It is held until the main agent has streamed no text or thought for `--entry-gap-ms` (default 600), or forwards a tool call, or the turn ends (the entry then goes out before the prompt response), or no prompt is outstanding. `--no-finish-entry` disables it.

### Roster

Zed shows the ACP `plan` in its activity bar and replaces the whole plan on every `plan` update. pons keeps the adapter's latest plan and appends one entry per background subagent (`<subagent_type> - <description>: <last action> (3 tool calls, 15s)`, status `pending`, `in_progress` or `completed`), sending the merged plan on change at most once a second. Finished subagents stay listed as `completed` until a turn that began after they finished ends, or 60 s pass. A plan from the adapter while no subagent is known is forwarded untouched.

The label comes from the Agent call's input: `subagent_type` (default `general-purpose`), the `description` (else the first line of `prompt`), with a `model` input shown in parentheses after the type, e.g. `worker (sonnet) - Fix ledger torn tail`. It is capped at 100 characters. Without that input the card title is used. `--no-roster` disables it.

### Notices

When a subagent finishes, pons can also send a session `notice` (severity `info`, or `warning` after a Stop; title `<title> finished`). Zed shows it above the composer until the user dismisses it. ACP notices have no ID, so pons cannot dismiss or replace one. By default pons sends a notice only to a client that advertised the `session.notices` capability, which Zed does since 1.25.2. `--notices` sends it regardless, and `--no-notice` never does. The roster still shows finished subagents as `completed` either way.

## Task notifications and interrupt markers

After a restart Zed replays the session, and the adapter (depending on its version) can send the background tasks' raw `<task-notification>...</task-notification>` text as `user_message_chunk`s, which Zed shows as a block of XML in the chat. pons holds a user message that starts with that tag until its closing tag and drops it. Other user text is forwarded as it came. Held text that turns out not to be a notification, or is not completed within a second, is released unchanged. `--keep-task-notifications` forwards them.

Claude Code records an interrupt as a user message `[Request interrupted by user]` (or `... for tool use]`), which the adapter replays after a restart, so the resumed chat shows a message you never typed. While a session is being replayed (`session/load` or `session/resume`, until its response) pons drops a user message whose trimmed text is exactly one of the two, also when split over chunks, and counts them in the `replay of ... done` log line. Other text is forwarded at once, unchanged. Live text is never dropped. `--keep-interrupt-markers` forwards them.

## Wire log

Every message is appended as one JSON object per line, with these keys:

- `ts` (seconds), `ms` (milliseconds within the second) and `level`.
- `event`: `wire` for a message, `note` for pons' own decisions.
- `dir`: `client->agent`, `agent->client`, `pons->client` or `pons`.
- `text`: the same text the log showed before.

Without `--log`, each start writes a new file `pons-<local time>-<pid>.jsonl` in the log directory: `~/Library/Logs/pons` on macOS, `$XDG_STATE_HOME/pons` (else `~/.local/state/pons`) on Linux, and `%LOCALAPPDATA%\pons\logs` on Windows. At startup the newest 10 are kept and older `pons-*.jsonl` files are deleted, and so are older `pons-*.log` text logs. Other files are never touched. `--log-dir DIR` changes the directory and `--no-log` turns logging off. `--log FILE` (or `PONS_LOG`) writes that one file instead.

`--log-level` picks the detail:

- `all` logs everything verbatim.
- `default` (the default) logs every message, but reduces `session/update` notifications to one line (kind, toolCallId, status, parentToolUseId, text length). Requests and responses stay full.
- `meta` logs only pons' own decisions plus method names.

An explicit `--log` file that would exceed `--log-max-bytes` (default 5 MiB, 0 for no cap) is moved to `FILE.old`, replacing an older one, and a fresh file starts. Default per-start logs are not rotated. Warnings and errors are written at once. Other lines are buffered (8 KiB) and written when the buffer fills, when a warning or error is logged, and when pons exits, so a log read while pons runs can lag behind.

One line of the log, wrapped here for reading:

```json
{
    "ts": 1793623634,
    "level": "info",
    "event": "wire",
    "ms": 268,
    "dir": "client->agent",
    "text": "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/cancel\",\"params\":{\"sessionId\":\"s1\"}}"
}
```

To list the warnings and errors of a log:

```sh
jq -r 'select(.level != "info") | "\(.ts) \(.level) \(.text)"' ~/.local/state/pons/pons-*.jsonl
```

`pons self doctor` summarizes the newest log (see [self doctor](#pons-self-doctor)).

## Adapter workarounds

These cover gaps in the Claude adapter (0.85.0) that leave Zed showing a turn as running for good. Each has a flag, and each decision is written to the log.

- **Turn watchdog** (`--turn-stall-ms`, default 120000, 0 disables). The adapter sometimes never answers a `session/prompt` (adapter issues #1284, #896). When a prompt is outstanding, the agent's last word was assistant text or a `usage_update`, no tool call, subagent card or question to the client (permission, elicitation) is open, and nothing at all arrived for the whole window, pons answers the prompt with `stopReason: "end_turn"`. The id goes on the swallow list, so a late real response is dropped. pons is conservative on purpose: a tool call that ended and then silence, a thought after the text, a running or provisionally finished subagent, a steering in flight or any update inside the window means no answer. A model that thinks silently for longer than the window right after streaming text is the one case it would get wrong; raise the value or use 0 then. Off while pons steps aside.
- **Ghost tool calls** (`--no-close-ghost-calls` keeps the old behaviour). Cancelled turns leave tool calls without a terminal state (#1061). pons tracks the open main-agent tool calls (not subagent cards) and, when a turn ends cancelled by a real Stop (the agent's `cancelled` response, the held cancel that turned out to be a Stop, or the stop escape), sends `tool_call_update {status: "failed"}` for each of them once, before the response. A message sent while subagents run (cancel plus prompt) closes nothing: the turn goes on.
- **Steering hold** (`--steer-hold-ms`, default 1500, 0 steers at once). Steering that lands while a tool call is still streaming its input fails in the adapter (`incomplete_tool_call`, #1267). pons treats a main-agent tool call as streaming while it is `pending` with an empty `rawInput` and has had neither a status change nor a refinement with `content`. It holds the steering until none is left, up to the time given, and then sends it. A held steering is queued instead (the prompt is forwarded) if the old turn ends meanwhile, and sent at once when a cancel or another prompt of the session arrives. A steering that is declined (`promptRequired`, or `noRunningTurn` when a task-notification cycle runs on its own, #1280) or fails with any error, `incomplete_tool_call` included, falls back to queueing as before.
- **Permission policy** (`--auto-approve none|allow-once|allow-always`, default `none`; `--auto-approve-tools LIST`). Opt in. With a mode set, a `session/request_permission` from the agent whose tool matches is answered by pons with the option of that kind (`allow-always` falls back to `allow_once`), and the client never sees the request. `LIST` is comma separated, matched case-insensitively as a prefix of the tool's title, kind or name (`Read,Grep,Glob`, `Bash`). With an empty list only the kinds `read`, `search`, `think` and `fetch` match. `edit`, `execute`, `delete` and `move` match only when listed. Requests for plan approval (`ExitPlanMode`) and questions (`AskUserQuestion`) are never answered. A request with no matching option, or for another tool, is forwarded untouched.
  **Security trade-off:** an auto-approved tool runs without any prompt in Zed and without you seeing it, and `allow-always` also makes the adapter remember the rule. A subagent's tool calls are covered by the same list, so `think` includes launching a subagent. `fetch` reads from the network. Use the narrowest list that helps you, keep `execute` and `edit` out unless you accept that the model may run them unattended, and read the log (`auto-approved permission request ...`).
- **Dead adapter.** If the agent process exits while prompts are outstanding (a crash, a usage limit, the adapter killed), Zed would wait forever. pons answers each of them with a JSON-RPC error (`pons: the agent process exited (code N); restart the thread`), sends one `agent_message_chunk` per session with the same text, and exits with the agent's status. It also exits now when the client keeps its stdin open (the exit used to wait for the client to close it). There is no respawn.
- **Not implemented: `<cc-memory>` tags.** The adapter 0.85.0 contains no `cc-memory` tag (searched `dist/` and its dependencies), so there is nothing to strip and no `--keep-cc-memory-tags` flag. Revisit if a later adapter version emits one.
- **Stdout draining.** pons reads the agent's stdout continuously in its own task and never waits for a write to the client. A client that does not read only grows the queue, up to 32 MiB (see [Shutdown](#shutdown)), and the agent is not stalled until then. An integration test floods 3 MiB with the client not reading.

## Shutdown

pons exits when Zed closes stdin and then shuts the agent down in steps. It closes the agent's stdin, waits `--shutdown-grace-ms` (default 2000) for it to exit, sends SIGTERM to the agent's whole process group, and sends SIGKILL after 2 more seconds, so grandchildren (the adapter's tools, subagent processes) go too. It exits with the agent's status when the agent dies, after sweeping what is left of its process group. It forwards SIGTERM, SIGHUP and SIGINT to the group, and forwards malformed lines raw. The reason for exiting is written to the log (`exiting: ...`). Memory stays bounded: each direction queues at most 32 MiB before pons stops reading from the sender, and per-session state is dropped on `session/close`.

## Stepping aside, replay and the fail-safe

- **Native subagents.** If the client's `initialize` request carries `clientCapabilities.subagents` (Zed's upstream subagent support advertises it), the client renders child sessions itself. pons then turns off folding, background-card rewriting, finished entries, the roster, notices and the cancel filter. Subagent traffic is pure passthrough, and pons logs one note (`stepping aside`). It keeps the wire log, the task-notification filter and the fail-safe. `--force-features` keeps the features on regardless.
- **Resumed threads.** Updates that arrive while a `session/load` or `session/resume` replays history (after the request, before its response or the next prompt) are forwarded untouched. pons does not register parents or background cards from them, so old cards are never "finished" in a burst by the idle heuristic, and no entries, notices or roster come from them. The one exception: with subagent text on, the adapter also replays old subagent text (updates with `parentToolUseId` and an `agent_message_chunk` or `agent_thought_chunk`), which would appear flat in the chat. pons drops those during the replay window and logs how many.
- **Fail-safe.** A rule that fails or panics on a message (the engine, fold, roster and notification filter) never breaks the session. pons forwards the original line unchanged and logs a `fail-safe:` note. After `--max-failures` (default 5) consecutive failures in one session, that session is pure passthrough. `0` never switches, and failures in messages without a session switch the whole proxy. Failures of the periodic work (finishing by silence, refreshes) are counted apart, logged, and never switch anything. Even in passthrough pons still swallows the late answer to a prompt it already answered. Build it with the workspace profile `release-unwind` (`panic = "unwind"`, see the root `Cargo.toml`). This catches panics with `catch_unwind`. State half-updated by a panic can only cause a missed rewrite, never a lost line.

## Options

```
pons [--log FILE | --no-log] [--log-dir DIR] [--log-level all|default|meta]
     [--log-max-bytes BYTES]
     [--cancel-defer-ms MS] [--interrupt-mode steer|queue|cancel]
     [--stop-window-ms MS] [--subagent-idle-ms MS]
     [--subagent-max-quiet-ms MS] [--shutdown-grace-ms MS]
     [--entry-gap-ms MS] [--notify-settle-ms MS]
     [--no-cancel-filter] [--no-fold] [--no-finish-entry] [--no-roster]
     [--notices | --no-notice] [--keep-task-notifications]
     [--keep-interrupt-markers] [--turn-stall-ms MS] [--no-close-ghost-calls]
     [--steer-hold-ms MS] [--auto-approve none|allow-once|allow-always]
     [--auto-approve-tools LIST]
     [--force-features] [--no-subagent-text] [--max-failures N] [--no-census]
     [--mcp-server NAME=COMMAND[,ARG...]]...
     [-- <agent> [args...]]
pons self doctor [--log FILE] [--log-dir DIR] [--agent-cmd CMD...]
pons self setup [--package-manager pnpm|bun|npm] [--dry-run]
pons self update [--check|--force]
pons self uninstall [--dry-run]
```

`pons --help` lists every flag with its default. The flags are explained in the sections above: [Cancel and steering](#cancel-and-steering) for the cancel and interrupt flags, [Subagents](#subagents) for the subagent and roster flags, [Task notifications and interrupt markers](#task-notifications-and-interrupt-markers) for the replay filters, [Wire log](#wire-log) for the log flags, [Adapter workarounds](#adapter-workarounds) for the adapter flags, and [Shutdown](#shutdown) for the grace periods.

`--no-census` (or `PONS_NO_CENSUS=1`) turns off the census record. Otherwise pons appends each subscription limit in the adapter's rate-limit events (the five-hour and seven-day windows) to the `snapshots` file of the census state directory whenever its value changes. That is `$XDG_STATE_HOME/census`, else `~/.local/state/census`, or `%LOCALAPPDATA%\census\state` on Windows. The limits are recorded only while pons applies its rules, so not in passthrough.

## Self

`pons self <action>` manages pons itself. The `self` commands take the output flags (`--agent`, `--human`, `--json`); see [Commands](#commands).

### pons self doctor

`pons self doctor [--json|--human|--agent] [--log FILE] [--log-dir DIR] [--agent-cmd CMD...]` prints a report. Its checks are in four groups (launcher, zed, processes, wire log), each `ok`, `warn` or `fail`, followed by the details behind them. It exits 1 when a check fails and 0 when none does. `--json` prints one JSON line in the shared doctor shape: `tool`, `version`, `ok` (false when a check fails) and `groups`, where each check has `name`, `status`, `detail` and, when it has one, `hint`. `--human` colours the text on a terminal. `--agent` is the plain default. The output flags also work on `pons self` itself.

The report covers:

- **Launcher:** whether the Claude ACP adapter can be started, which launcher would run it, its version when known, and, when none can, the install command for pnpm, bun or npm on `PATH`. It lists every candidate's resolved path, and the version the launcher resolves to, read from its `package.json` and never by starting it. That version is labelled apart from the adapter Zed installed under `external_agents`, which pons would not use unless the command points at it.
- **Zed:** whether Zed's `settings.json` (`~/.config/zed/` on macOS and Linux, `%APPDATA%\Zed\` on Windows) has an `agent_servers` entry running pons. It is parsed leniently as JSONC, so a parse failure only changes the wording.
- **Processes:** the running pons processes (Unix), flagged `STALE` when one started before the installed binary was replaced and so still runs old code.
- **Wire log:** the log given by `--log`, `PONS_LOG`, else the newest per-start log in the log directory, else `pons.log` in the temp directory. It shows the size and rotated copy, cancels dropped, forwarded and stop escapes, subagents tracked, finished and provisionally finished, fail-safe events, dropped task notifications, why pons last exited, and the last 5 notes.

### pons self setup

`pons self setup [--package-manager pnpm|bun|npm] [--dry-run] [--json|--human|--agent]` makes sure the Claude ACP adapter is installed. When `claude-agent-acp` is on `PATH` (or in one of the user bin directories pons searches), it prints its path and version and exits 0. Otherwise it installs `@agentclientprotocol/claude-agent-acp` globally, with the package manager you name or else the first of pnpm, bun or npm that is found. `--dry-run` prints the install command and installs nothing. When no package manager is found, it exits 2 and the hint says what to install. When the install command fails, the error names the command, and the hint says to run `pons self setup --dry-run` to see it, or to pick another manager with `--package-manager`.

### pons self update

`pons self update [--check|--force]` replaces the binary with the latest GitHub release. `--check` only reports whether a newer release exists. `--force` reinstalls the latest release even when it is not newer. Run the install command again if you prefer that route.

### pons self uninstall

`pons self uninstall [--dry-run]` removes this binary. `--dry-run` only lists what would go. The wire logs and the Zed `agent_servers` entry stay. The installers remove it too, with `install.sh --uninstall pons` or `.\install.ps1 -Uninstall pons` (see [Install](../README.md#install) in the capsa README). Neither removes the `PATH` line the installer added. Remove the line after `# added by capsa installer` in your shell profile on macOS and Linux, or `/etc/profile.d/capsa.sh` with `CAPSA_INSTALL_SCOPE=system`. On Windows, remove the directory from your user `PATH`. A binary installed with Cargo is removed with `cargo uninstall pons`.

## Development

```sh
cargo build -p pons --profile release-unwind
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks on Ubuntu, macOS and Windows. For the layout and conventions, see [AGENTS.md](AGENTS.md). Release notes go in [CHANGELOG.md](CHANGELOG.md); the release process is described under [Releases](../README.md#releases) in the capsa README.

## License

[MPL-2.0](../LICENSE).

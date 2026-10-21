# pons

![Rust](https://img.shields.io/badge/rust-2024-orange.svg)
![macOS](https://img.shields.io/badge/macOS-primary-black.svg)
![Linux](https://img.shields.io/badge/Linux-CI-lightgrey.svg)
![Windows](https://img.shields.io/badge/Windows-CI-lightgrey.svg)
[![License](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](LICENSE)

A transparent stdio proxy between Zed (ACP client) and the Claude ACP adapter
(`@agentclientprotocol/claude-agent-acp`). It forwards every message unchanged
except for the Zed + Claude subagent problems below:

1. **Cancel filter and steering.** Zed sends `session/cancel` before each new
   `session/prompt` while a prompt is unresolved (~100 ms apart). The adapter
   keeps a turn open while background subagents run, and an interrupt kills
   them. So when a prompt is outstanding and at least one subagent is in
   flight, pons does not forward the cancel at once, whether or not the main
   agent is streaming. It answers the pending prompt itself with
   `stopReason: "cancelled"` (the agent's later response to it is swallowed, so
   Zed can go on) and holds the cancel for `--cancel-defer-ms` (default 400):
   - a `session/prompt` inside the window means the user sent a message: the
     cancel is dropped for good and the subagents live on. The message is
     delivered by `--interrupt-mode`: `steer` (default if the adapter
     advertised `_meta.steering.supported` at `initialize`) sends the adapter's
     `_session/steering {sessionId, prompt}` request instead of the prompt, so
     the text is injected into the running turn; `queue` (the default without
     steering, and the fallback when steering fails or is declined) forwards
     the prompt unchanged and the adapter queues it behind the running turn;
     `cancel` is the old behavior (the cancel is forwarded, which kills the
     subagents);
   - no prompt inside the window means a real Stop: the subagent cards are
     marked stopped and the cancel is forwarded to the agent;
   - **Stop escape:** a second `session/cancel` within `--stop-window-ms`
     (default 3000) of a held one is forwarded at once. `--stop-window-ms 0`
     disables the escape (the held cancel then waits for its window).

   With steering, Zed's new prompt id is not sent to the agent. The real turn
   runs under the old prompt id (already answered as cancelled), so pons keeps
   the new id open and, when the agent answers the old prompt, answers the new
   one with that same result. Updates stream meanwhile; the cards keep running
   and the background cards of the old turn finish with the new prompt's
   response. The state machine:

   ```text
   idle --cancel (prompt + subagents)--> WINDOW  (old prompt answered cancelled)
   WINDOW --second cancel------------->  stop forwarded at once
   WINDOW --window over--------------->  stop: cards stopped, cancel forwarded
   WINDOW --prompt-------------------->  message: cancel dropped
       steer: send _session/steering (id pons-steer-N), wait for the reply
         injected        new prompt waits; the old prompt's response answers it
         promptRequired,
         error, other    forward the prompt (queue)
         startedNewTurn  the adapter runs it detached: answer end_turn, no forward
       queue: forward the prompt (the old prompt's response is swallowed)
   ```

   The steering request carries `_meta.steering.idleBehavior = "promptRequired"`
   so an idle adapter declines instead of starting a turn nobody answers.
   Without subagents a cancel is forwarded at once, as before.
   `--quiet-ms` is deprecated and ignored.
2. **Subagent folding.** Subagent `session/update`s (those carrying
   `_meta.claudeCode.parentToolUseId`) are folded into the parent Task/Agent
   tool call as a throttled `tool_call_update` with a compact text log
(`running: Read src/a.rs`, `done: ...`, `failed: ...`, `agent: last agent
text`, last 30 lines). pons only emits plain ASCII.
   **Subagent text.** The adapter sends a subagent's text to the client only if
   the client's `initialize` request advertises `clientCapabilities._meta
   ["subagent-transcript"] = true` (or native `subagents`); Zed advertises
   neither, so without help pons sees only tool calls. pons adds that
   capability to the `initialize` it forwards (creating `clientCapabilities`
   and `_meta` if absent, logging one note), unless it steps aside, the client
   already set the key, or `--no-fold` / `--no-subagent-text` is given. The
   subagent's text is then folded like its tool calls and never forwarded flat.
   Permission requests are never touched unless you opt in with `--auto-approve`
   (see Options): then pons answers the requests of the matching tools itself.
   Elicitations are never touched. Any tool call that a
   child names in `parentToolUseId` is a parent, whatever its name.
   **Background subagents.** The adapter marks an `Agent` call completed the
   moment it launches a background subagent ("Async agent launched
   successfully") and sends no completion event to clients without native
   subagent support, which includes Zed. pons rewrites that update to
`in_progress` (`running in the background`), keeps folding the subagent's
activity into the card, and finishes it with a last log line and status
`completed`: `done - N tool calls`. The signals it can observe are, in
order of strength:
   - **Prompt response (confirmed).** The adapter holds a `session/prompt`
     open until the subagents that turn spawned are gone, so a response to a
     prompt that is still the newest one finishes the background cards that
     prompt launched. A response that only arrives because a newer prompt took
     over (a handoff) finishes nothing; the cards then belong to the newer one.
   - **Task notification (confirmed).** The adapter's `task-notification`
     usage update is the only thing it puts on the wire when one background
     subagent ends, and it does not say which. pons attributes it to a card with
     no open tool call, preferring one whose last event was its own text, then
     the one quiet the longest (`--notify-settle-ms`, default 0, can delay the
     decision). With no such card it finishes nothing and only logs that.
   - **Silence (provisional).** A card whose last event was the subagent's
     own text, with no tool call open, and no activity for `--subagent-idle-ms`
     (default 20000, 0 disables) is finished as
     `done (no activity for 20s) - N tool calls`. A card whose last event was a
     tool result or thinking, or that has a tool call open, is never finished
     this way, because a subagent thinks or runs a long tool silently; its title
     and roster line show `quiet 1m 5s` instead. With `--no-subagent-text`
     pons cannot see text, so only `--subagent-max-quiet-ms` (default 600000, 0
     disables) ends a silent card. Silent finishes are provisional: no notice,
     the cancel filter keeps protecting the card, and later activity reopens
     it as a background card with its log.
   - **Stop.** A real Stop marks the running cards `failed`
     (`stopped - N tool calls`).

   Running background subagents also hold the cancel filter across turns.
   **Card title.** A background card's `title` carries live counters:
`<title>: running 15s, 3 tool calls, last: Read src/a.rs` (elapsed time
measured by pons from the launch, refreshed at most every 5 s, the rest on
the fold throttle), and at the end `<title>: done, 3 tool calls, 42s`
(`stopped` after a Stop).
   **Finished entry.** When pons finishes a card it also emits a new tool call
(`pons-finished-<parent>`, one stable id per card, kind `other`, title
`<title> finished`, or `<title> stopped` with status `failed` on Stop). If the
card turns out to be alive after the entry was sent, the entry is updated
(`in_progress`, `resumed`) and, on the next finish, completed with the result;
a second entry is never appended. Its text is the tool call
count, then the subagent's final message (its last contiguous agent text, at
most 2000 characters, the head kept and cut with `...`), so Zed shows the
result at the end of the thread:

   ```
   Review the diff finished
   3 tool calls
   Found 2 issues.
   1. src/a.rs:10 unused import
   2. src/b.rs:4 missing test
   ```

   The finished card shows the same message as its last `agent:` line; a
   running card shows the latest 300 characters of the current text.
The entry never splits the main agent's text: it is held until the main agent
streamed no text or thought for `--entry-gap-ms` (default 600), or forwards a
tool call, or the turn ends (the entry goes out before the prompt response),
or no prompt is outstanding. `--no-finish-entry` disables it.
   **Roster.** Zed shows the ACP `plan` in its activity bar and replaces the
whole plan on every `plan` update. pons keeps the adapter's latest plan and
appends one entry per background subagent (`<title>: <last action> (3 tool
calls, 15s)`, status `pending`, `in_progress` or `completed`), sending the
merged plan on change at most once a second. Finished subagents stay listed as
`completed` until a turn that began after they finished ends, or 60 s pass. A plan from the
adapter while no subagent is known is forwarded untouched. `--no-roster`
disables it.
   **Notices.** When a subagent finishes, pons can also send a session
`notice` (severity `info`, or `warning` after a Stop; title `<title> finished`;
Zed shows it above the composer until dismissed). Zed 1.23.2 renders notices
without checking its beta flag, but only advertises the `session.notices`
capability with it on, so by default pons sends a notice only to a client that
advertised the capability. `--notices` sends it regardless (what to use with
Zed's stable release), `--no-notice` never does.
3. **Task notifications.** After a restart Zed replays the session and the
adapter (depending on its version) can send the background tasks' raw
`<task-notification>...</task-notification>` text as `user_message_chunk`s,
which Zed shows as a block of XML in the chat. pons holds a user message that
starts with that tag until its closing tag and drops it; other user text is
forwarded as it came, and held text that turns out not to be one (or is not
completed within a second) is released unchanged. `--keep-task-notifications`
forwards them.
   **Interrupt markers.** Claude Code records an interrupt as a user message
`[Request interrupted by user]` (or `... for tool use]`), which the adapter
replays after a restart, so the resumed chat shows a message you never typed.
While a session is being replayed (`session/load` / `session/resume`, until its
response) pons drops a user message whose trimmed text is exactly one of the two
(also when split over chunks; other text is forwarded at once, unchanged) and
counts them in the `replay of ... done` log line. Live text is never dropped.
`--keep-interrupt-markers` forwards them.
4. **Wire log.** Every message is appended with timestamp and direction
   (`client->agent`, `agent->client`, `pons->client`). Without `--log`, each
   start writes a new file `pons-<local time>-<pid>.log` in the log directory:
   `~/Library/Logs/pons` on macOS, `$XDG_STATE_HOME/pons` (else
   `~/.local/state/pons`) on Linux, `%LOCALAPPDATA%\pons\logs` on Windows. At
   startup the newest 10 are kept and older `pons-*.log` files are deleted;
   other files are never touched. `--log-dir DIR` changes the directory and
   `--no-log` turns logging off. `--log FILE` (or `PONS_LOG`) writes that one
   file instead, as before.
   `--log-level` picks the detail: `all` logs everything verbatim; `default`
   (the default) logs every message but reduces `session/update`
   notifications to one line (kind, toolCallId, status, parentToolUseId, text
   length) while requests and responses stay full; `meta` logs only pons'
   own decisions plus method names. An explicit `--log` file that would exceed
   `--log-max-bytes` (default 5 MiB, 0 for no cap) is moved to `FILE.1`
   (replacing an older one) and a fresh file starts. Default per-start logs are
   not rotated.
5. **Adapter workarounds.** Gaps in the Claude adapter (0.85.0) that leave Zed
   showing a turn as running for good. Each has a flag, and each decision is
   written to the log.
   - **Turn watchdog** (`--turn-stall-ms`, default 120000, 0 disables). The
     adapter sometimes never answers a `session/prompt` (adapter issues #1284,
     #896). When a prompt is outstanding, the agent's last word was assistant
     text or a `usage_update`, no tool call, subagent card or question to the
     client (permission, elicitation) is open, and nothing at all arrived for
     the whole window, pons answers the prompt with `stopReason: "end_turn"`.
     The id goes on the swallow list, so a late real response is dropped.
     pons is conservative on purpose: a tool call that ended and then silence,
     a thought after the text, a running or provisionally finished subagent, a
     steering in flight or any update inside the window means no answer. A
     model that thinks silently for longer than the window right after
     streaming text is the one case it would get wrong; raise the value or
     use 0 then. Off while pons steps aside.
   - **Ghost tool calls** (`--no-close-ghost-calls` keeps the old behavior).
     Cancelled turns leave tool calls without a terminal state (#1061). pons
     tracks the open main-agent tool calls (not subagent cards) and, when a
     turn ends cancelled by a real Stop (the agent's `cancelled` response, the
     held cancel that turned out to be a Stop, or the stop escape), sends
     `tool_call_update {status: "failed"}` for each of them once, before the
     response. A message sent while subagents run (cancel plus prompt) closes
     nothing: the turn goes on.
   - **Steering hold** (`--steer-hold-ms`, default 1500, 0 steers at once).
     Steering that lands while a tool call is still streaming its input fails
     in the adapter (`incomplete_tool_call`, #1267). pons treats a main-agent
     tool call as streaming while it is `pending` with an empty `rawInput` and
     has had neither a status change nor a refinement with `content`; it holds
     the steering until none is left, up to the time given, and sends it then.
     A held steering is queued instead (the prompt is forwarded) if the old
     turn ends meanwhile, and sent at once when a cancel or another prompt of
     the session arrives. A steering that is declined (`promptRequired`,
     `noRunningTurn` when a task-notification cycle runs on its own, #1280) or
     fails with any error, `incomplete_tool_call` included, falls back to
     queueing as before.
   - **Permission policy** (`--auto-approve none|allow-once|allow-always`,
     default `none`; `--auto-approve-tools LIST`). Opt in. With a mode set,
     a `session/request_permission` from the agent whose tool matches is
     answered by pons with the option of that kind (`allow-always` falls back
     to `allow_once`), and the client never sees the request. `LIST` is comma
     separated, matched case-insensitively as a prefix of the tool's title,
     kind or name (`Read,Grep,Glob`, `Bash`). With an empty list only the
     kinds `read`, `search`, `think` and `fetch` match; `edit`, `execute`,
     `delete` and `move` only when listed. Requests for plan approval
     (`ExitPlanMode`) and questions (`AskUserQuestion`) are never answered. A
     request with no matching option, or for another tool, is forwarded
     untouched. **Security trade-off:** an auto-approved tool runs without any
     prompt in Zed and without you seeing it; `allow-always` also makes the
     adapter remember the rule. A subagent's tool calls are covered by the
     same list, so `think` includes launching a subagent. `fetch` reads from
     the network. Use the narrowest list that helps you, keep `execute` and
     `edit` out unless you accept that the model may run them unattended, and
     read the log (`auto-approved permission request ...`).
   - **Dead adapter.** If the agent process exits while prompts are
     outstanding (a crash, a usage limit, the adapter killed), Zed would wait
     forever. pons answers each of them with a JSON-RPC error (`pons: the agent
     process exited (code N); restart the thread`), sends one
     `agent_message_chunk` per session with the same text, and exits with the
     agent's status. It also exits now when the client keeps its stdin open
     (the exit used to wait for the client to close it). No respawn.
   - **Not implemented: `<cc-memory>` tags.** The adapter 0.85.0 contains no
     `cc-memory` tag (searched `dist/` and its dependencies), so there is
     nothing to strip and no `--keep-cc-memory-tags` flag. Revisit if a later
     adapter version emits one.
   - **Stdout draining.** pons reads the agent's stdout continuously in its
     own task and never waits for a write to the client: a client that does not
     read only grows the queue (up to 32 MiB, see below), and the agent is not
     stalled until then (an integration test floods 3 MiB with the client not
     reading).

pons exits when Zed closes stdin and then shuts the agent down in steps: it
closes the agent's stdin, waits `--shutdown-grace-ms` (default 2000) for it to
exit, then sends SIGTERM to the agent's whole process group, and SIGKILL after
2 more seconds, so grandchildren (the adapter's tools, subagent processes) go
too. It exits with the agent's status when the agent dies (after sweeping what
is left of its process group), forwards SIGTERM/SIGHUP/SIGINT to the group, and
forwards malformed lines raw. The reason for exiting is written to the log
(`exiting: ...`). Memory stays bounded: each direction queues at most 32 MiB
before pons stops reading from the sender, and per-session state is dropped on
`session/close`.

## Stepping aside, replay and the fail-safe

- **Native subagents.** If the client's `initialize` request carries
  `clientCapabilities.subagents` (Zed's upstream subagent support advertises
  it), the client renders child sessions itself. pons then turns off folding,
  background-card rewriting, finished entries, the roster, notices and the
  cancel filter, so subagent traffic is pure passthrough, logs one note
  (`stepping aside`), and keeps the wire log, the task-notification filter and
  the fail-safe. `--force-features` keeps the features on regardless.
- **Resumed threads.** Updates that arrive while a `session/load` or
  `session/resume` replays history (after the request, before its response or
  the next prompt) are forwarded untouched. pons does not register parents or
  background cards from them, so old cards are never "finished" in a burst by
  the idle heuristic and no entries, notices or roster come from them. The one
  exception: with subagent text on, the adapter also replays old subagent text
  (updates with `parentToolUseId` and an `agent_message_chunk` or
  `agent_thought_chunk`), which would appear flat in the chat; pons drops those
  during the replay window and logs how many.
- **Fail-safe.** A rule that fails or panics on a message (the engine, fold,
  roster and notification filter) never breaks the session: pons forwards the
  original line unchanged and logs a `fail-safe:` note. After
  `--max-failures` (default 5) consecutive failures in one session that
  session is pure passthrough (0 never switches; failures in messages without
  a session switch the whole proxy). Failures of the periodic work (finishing
  by silence, refreshes) are counted apart, logged, and never switch anything.
  Even in passthrough pons still swallows the late answer to a prompt it
  already answered. Build it with the workspace profile `release-unwind`
  (`panic = "unwind"`, see the root `Cargo.toml`): this catches panics with
  `catch_unwind`; state
  half-updated by a panic can only cause a missed rewrite, never a lost line.

## pons self doctor

`pons self doctor [--json|--human|--agent] [--log FILE] [--log-dir DIR] [--agent-cmd CMD...]` prints a report and
exits 0 (`--json` prints the same data as one JSON object; `--human` colours
the text on a terminal, `--agent` is the plain default). It also says whether
the Claude ACP adapter can be started: which launcher would run it, its version
when known, and, when none can, the install command for pnpm, bun or npm on
`PATH`. The report lists pons version and
platform; the launcher that would be used and the resolved path of every
candidate; the version of the adapter that launcher resolves to (read from its
`package.json`, never by starting it), labeled apart from the adapter Zed
installed under `external_agents`, which pons would not use unless the command
points at it; whether Zed's `settings.json` (`~/.config/zed/` on macOS and
Linux, `%APPDATA%\Zed\` on Windows) has an `agent_servers` entry running pons
(parsed leniently as JSONC; a parse failure only changes the wording); the
running pons processes (Unix), flagged `STALE` when one started before the
installed binary was replaced and so still runs old code; and a summary of the
wire log (`--log`, `PONS_LOG`, else the newest per-start log in the log
directory, else `pons.log` in the temp directory): its
size and rotated copy, cancels dropped, forwarded and stop escapes, subagents
tracked, finished and provisionally finished, fail-safe events, dropped task
notifications, why pons last exited, and the last 5 notes.

## pons self setup

`pons self setup [--package-manager pnpm|bun|npm] [--dry-run] [--json|--human|--agent]` makes sure the Claude ACP adapter is installed. When `claude-agent-acp` is on `PATH` (or in one of the user bin directories pons searches), it prints its path and version and exits 0. Otherwise it installs `@agentclientprotocol/claude-agent-acp` globally: with the package manager you name, or else the first of pnpm, bun or npm that is found. `--dry-run` prints the install command and installs nothing. When no package manager is found, it exits 2 and the hint says what to install.

## Install

pons is distributed from the private repository [LuMiSxh/capsa](https://github.com/LuMiSxh/capsa). Its root `install.sh` and `install.ps1` need a GitHub token with read access: set `GITHUB_TOKEN`, for example `export GITHUB_TOKEN=$(gh auth token)` after `gh auth login`.

macOS and Linux, pons only:

```sh
curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" \
  https://raw.githubusercontent.com/LuMiSxh/capsa/main/install.sh | sh -s -- pons
```

Windows (PowerShell), pons only:

```powershell
$h = @{ Authorization = "Bearer $env:GITHUB_TOKEN" }
& ([scriptblock]::Create((irm -Headers $h https://raw.githubusercontent.com/LuMiSxh/capsa/main/install.ps1))) pons
```

The argument picks the tools: `pons`, `acus`, `velum` (macOS only) or `all`.
Both installers download the newest release of each tool to `~/.local/bin`
(`CAPSA_INSTALL_DIR` changes that), check it against its `.sha256` file, add
that directory to `PATH`. `CAPSA_INSTALL_SCOPE=system` adds it system-wide
instead (needs root on Linux and macOS, an elevated shell on Windows).

## Update

Run the install command again to get the newest release.

From source, with the Rust toolchain pinned in the repo's `rust-toolchain.toml`
(the newest stable, currently 1.99.1), from a checkout of the capsa repo root:

```sh
cargo install --path pons
```

This puts `pons` in `~/.cargo/bin`. The former repository, `LuMiSxh/pons`, is
frozen; build from capsa.

## Uninstall

The root installer has no uninstall. Remove `pons` from the install directory
(`~/.local/bin`, or `CAPSA_INSTALL_DIR`) and remove the `PATH` line the installer
added: the line after `# added by capsa installer` in your shell profile on
macOS and Linux, or `/etc/profile.d/capsa.sh` with `CAPSA_INSTALL_SCOPE=system`.
On Windows, remove the directory from your user `PATH`. A binary installed with
Cargo is removed with `cargo uninstall pons`.

## Platforms

Release binaries: macOS (arm64, x86_64), Linux (x86_64, arm64) and Windows (x86_64). macOS is the primary platform; CI builds and tests on all three. On Unix the agent runs in its own process group, and SIGTERM, SIGHUP and SIGINT are forwarded to the group, which is killed if it lingers for 5 seconds. On Windows there are no process groups or SIGTERM: after the shutdown grace period (or on Ctrl+C) the agent is ended with `taskkill /T /F`, which also takes its child processes (a Job object would be tighter; this path is untested by the maintainer).

## Zed

With `pons` on `PATH` (the installers above), in `settings.json`, the minimal
form needs neither `--` nor the agent path:

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

Each start writes its own wire log (see [Options](#options)). To write one fixed
file instead, use `"args": ["--log", "/tmp/pons.log"]`.

Zed may not inherit your shell `PATH` when launched from the Dock or Start menu. If it cannot find `pons`, set `command` to the absolute path instead, such as `/Users/you/.cargo/bin/pons` or `/Users/you/.local/bin/pons` (Zed does not expand `~`).

### Agent command

Without `-- <agent> [args...]` pons picks the first of these it can find:

1. `claude-agent-acp`
2. `npx -y @agentclientprotocol/claude-agent-acp`
3. `pnpm dlx @agentclientprotocol/claude-agent-acp` (or `pnpx @agentclientprotocol/claude-agent-acp`)
4. `bunx @agentclientprotocol/claude-agent-acp`

Each program is searched on `PATH` first, then in the user directories Zed's `PATH` often lacks: `~/.local/bin`, `~/.cargo/bin`, `~/Library/pnpm` and `~/Library/pnpm/bin`, `~/.local/share/pnpm` and its `bin`, `~/.bun/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `~/.volta/bin`, fnm's default node `bin`, and every nvm node `bin` (newest first). On Windows: `%APPDATA%\npm`, `%LOCALAPPDATA%\pnpm` (and `\bin`), `%USERPROFILE%\.bun\bin`, `%LOCALAPPDATA%\Volta\bin`, `%USERPROFILE%\.cargo\bin`, and `PATHEXT` extensions are tried (`npx.cmd`, `bunx.exe`; a bare `npx` script is not runnable there). When pons locates a program itself, the agent's `PATH` gets that program's directory and the existing user directories appended, so a `#!/usr/bin/env node` shim finds `node`. An explicit command after `--` always wins (a bare name is resolved the same way). On Windows, `.cmd` and `.bat` shims are spawned through `cmd.exe /c` by Rust's `Command`, which escapes their arguments and refuses what it cannot make safe; pons additionally rejects arguments containing `"`, `%`, `^`, `&`, `|`, `<`, `>`, `!`, a backtick or a line break for such a shim. `pons self doctor` shows what would be chosen.

The environment (including Anthropic auth) is inherited by the agent process.

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
     [--force-features] [--no-subagent-text] [--max-failures N]
     [-- <agent> [args...]]
pons self doctor [--json|--human|--agent] [--log FILE] [--log-dir DIR] [--agent-cmd CMD...]
pons self setup [--package-manager pnpm|bun|npm] [--dry-run]
pons self update [--check|--force]
pons self uninstall [--dry-run]
```

`pons --help` lists every flag with its default.

## Development

```sh
cargo build -p pons --profile release-unwind
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks on Ubuntu, macOS and Windows. Releases are built by the `Release` workflow: bump the version in `Cargo.toml`, add a `CHANGELOG.md` section, merge, then run the workflow with the tag (`v0.4.0`). It builds macOS (arm64, x86_64), Windows (x86_64) and Linux (x86_64, arm64) archives with checksums.

## License

[MPL-2.0](LICENSE).

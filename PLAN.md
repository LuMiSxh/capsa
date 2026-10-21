# capsa plan

capsa is a Cargo workspace for agent tooling. Each tool keeps its own CLI and
version and can be installed alone or as a suite:

| Tool    | Purpose                                                            | Status                                  |
|---------|--------------------------------------------------------------------|-----------------------------------------|
| `velum` | Thin policy shield that runs coding agents in a macOS sandbox      | new                                     |
| `pons`  | ACP proxy between Zed and the Claude ACP adapter                   | moves in from `LuMiSxh/pons` (private)  |
| `acus`  | Code search, outline and patch CLI for agents                      | moves in later from `LuMiSxh/Acus`      |

## Goal

Claude Code keeps running with `--dangerously-skip-permissions`, and from the
outside nothing changes: `claude` in the terminal and the `claude-pons` agent
in Zed work as before, with the subscription login. Internally the whole
`claude` process runs inside a sandbox whose rules we define globally, per
repository and per session. Repositories stay in `~/Documents/projects`.

Non-goals for the first version: a VM backend, Linux and Windows support,
hiding paths behind decoy contents.

## velum

### Engine

velum does not implement a sandbox. It uses Anthropic's sandbox-runtime
(`srt`, npm `@anthropic-ai/sandbox-runtime`, Apache-2.0, beta), which wraps a
whole process tree in Seatbelt on macOS and routes network traffic through its
own proxy. Unlike Claude Code's built-in `/sandbox`, which covers only Bash,
this also covers the Read/Edit/Write tools, hooks and MCP servers.

velum's job:

1. merge the policy layers into an srt settings file,
2. prepare the environment (allowlisted variables, `TMPDIR`, flags for tools
   that sandbox themselves),
3. run `srt -s <generated> -- claude --dangerously-skip-permissions …`,
4. decide unknown network destinations by asking the user,
5. install and update srt with the user's package manager.

### Policy

Layers, later ones extend earlier ones: built-in defaults → global file →
repository section → grants given during a session.

```toml
# ~/.config/velum/policy.toml
[fs]
write      = ["~/Documents/projects", "~/.cargo", "~/Library/Caches", "~/Library/Developer"]
deny_read  = ["~/.ssh", "~/.aws", "~/.config/secrets.env", "~/Library/Mail", "~/Library/Messages"]
deny_write = ["**/.git/hooks", "**/.git/config"]

[env]
pass = ["PATH", "HOME", "USER", "LANG", "TERM", "COLORTERM", "SHELL", "CARGO_*", "RUSTUP_*"]

[net]
allow   = ["api.anthropic.com", "*.anthropic.com", "claude.ai", "platform.claude.com",
           "github.com", "*.githubusercontent.com", "crates.io", "static.crates.io",
           "index.crates.io", "registry.npmjs.org", "pypi.org", "files.pythonhosted.org"]
unknown = "ask"   # ask | deny | allow

[repo."Arknights-MacOS-Client"]
fs.write  = ["~/Library/Developer/Xcode/DerivedData"]
net.allow = ["developer.apple.com"]
```

Per-repository rules live in the global file, keyed by repository. A policy
file inside a repository is writable by the agent, so it would let the agent
grant itself access. If in-repo files are added later, they apply only after
the user approves them, pinned by hash, and every change asks again.

### Protecting the host from the agent's own configuration

The agent needs to write `~/.claude` (sessions, todos, memory) and
`~/.claude.json` (state). Both also hold configuration that runs code:
hooks in `settings.json`, MCP servers in `~/.claude.json`, agents, skills and
plugins. Anything the agent adds there runs on the next start. This stays
contained only if:

- every Claude start goes through velum: the terminal shim and Zed (via
  `pons -- velum claude-agent-acp`), and
- `~/.claude/settings.json`, `~/.claude/CLAUDE.md`, `~/.claude/agents`,
  `~/.claude/skills`, `~/.claude/plugins`, `~/.claude/output-styles` and
  `~/.claude/hooks` are in `deny_write`.

`~/.claude.json` cannot be made read-only without breaking Claude Code. Open
question: whether srt can protect the `mcpServers` keys, or velum must check
and restore them after each session.

### Network prompts

srt offers three hooks, all confirmed in its README (0.0.81):

- `SandboxManager.initialize(config, askCallback)` asks about a destination
  that neither list matches,
- `srt --control-fd <fd>` takes config updates while the command runs,
- `srt proxy` runs only the proxy and lets an external decider process
  answer each request.

velum answers unknown hosts with a native macOS dialog: allow once, always for
this repository, always, or deny. Permanent answers are written back to
`policy.toml`. The connection waits while the dialog is open, with a timeout
that denies.

### Host file requests

Paths outside the policy cannot be mounted into a running sandbox. velum ships
a small MCP server with `request_host_access(path, mode, reason)`. A dialog
shows the path and reason. On approval velum either extends the policy for
the session through `--control-fd` (reads) or copies the file into an inbox
and back from an outbox after showing a diff (writes). The agent can request;
only the user decides.

### Installation

`velum setup` installs srt with the configured package manager (default
`pnpm`, also `npm` and `bun`), writes the default policy, installs the `claude`
shim earlier on `PATH` than the real binary, and prints what it changed.
`velum doctor` checks srt, the shim, the login and the Zed entry. Both are
idempotent and have `--dry-run`.

## Spike results (2026-10-20, macOS 27.0.1, srt 0.0.81, Claude Code 2.1.304)

| Check                                          | Result                                                     |
|------------------------------------------------|------------------------------------------------------------|
| Write inside an allowed path                   | works                                                      |
| Write to `~/Documents` outside allowed paths   | blocked, `Operation not permitted`                         |
| Read `~/.ssh`                                  | blocked                                                    |
| Read `~/Documents/projects`                    | works                                                      |
| HTTPS to a host not on the allowlist           | blocked, proxy answers 403                                 |
| HTTPS to `api.anthropic.com`                   | works                                                      |
| Environment variables                          | all passed through; velum must filter                      |
| `claude auth status`                           | logged in via claude.ai (Keychain readable)                |
| `claude -p … --dangerously-skip-permissions`   | works                                                      |
| `cargo run`                                    | works                                                      |
| `swift build`                                  | works with `--disable-sandbox` and an existing `TMPDIR`    |
| `xcodebuild … build`                           | `BUILD SUCCEEDED` with `-IDEPackageSupportDisableManifestSandbox=1` |

Findings velum must handle:

- srt requires `network.deniedDomains` in the settings file, even when empty.
- srt sets `TMPDIR=/tmp/claude` but does not create it. Swift fails with
  `couldNotFindTmpDir`.
- Seatbelt cannot nest. SwiftPM and Xcode sandbox manifest compilation and
  fail with `sandbox_apply: Operation not permitted` unless their own sandbox
  is disabled. Other tools that call `sandbox-exec` will need the same.
- SwiftPM wants `~/Library/org.swift.swiftpm`.
- The Keychain is readable, which the login needs. Not yet tested: whether
  other Keychain items can be read without a macOS prompt. Test with a dummy
  item, never with real secrets.

## Monorepo

- Cargo workspace with one directory per tool, each self-contained (code,
  README, CHANGELOG, assets):

  ```text
  capsa/
    Cargo.toml            workspace, shared dependencies, profiles
    rust-toolchain.toml
    acus/                 crates/acus-*, skill/, docs/, scripts/
    pons/
    velum/
    core/                 capsa-core, once a second tool needs shared code
    .github/workflows/    one CI, one release workflow
    install.sh, install.ps1
  ```

- Independent versions and tags: `acus-v0.16.1`, `pons-v0.5.2`,
  `velum-v0.1.0`. Release assets keep the `<tool>-<target>.tar.gz|zip` plus
  `.sha256` naming acus already uses.
- acus and pons were copied on 2026-10-20 from their repositories (tracked
  files only, no history). From then on capsa is the only source and the
  only release location; `LuMiSxh/Acus` and `LuMiSxh/pons` are deleted (history bundles in `acus/.replay/`).
- `acus update` can no longer ask for `releases/latest`, because the latest
  release may belong to another tool. It lists releases and takes the newest
  `acus-v*` tag. While capsa is private, the GitHub API and `install.sh` need
  a token: `GITHUB_TOKEN`, else `gh auth token`.
- One installer that installs the suite or single tools.

### Shared crate `capsa-core`

Inventory of acus (0.16.1) and pons (0.5.2) on 2026-10-20. Both
use edition 2024, rust-version 1.88, anyhow, clap 4.6.7 derive and
serde_json 1.0.151. Neither shares a concurrency model: acus is synchronous
(`ignore` parallel walker, `std::thread::scope` pools, reader threads), pons
runs a tokio `current_thread` runtime. `capsa-core` therefore provides
building blocks, not a runtime. tokio-specific code sits behind an `rt`
feature so acus does not link it.

| Module    | Moves from                                                                                         | Used by           |
|-----------|----------------------------------------------------------------------------------------------------|-------------------|
| `exit`    | acus `main.rs` outcome mapping (0 found, 1 empty, 2 error with `hint:`), SIGPIPE reset              | all               |
| `paths`   | acus `config.rs#config_dir`, `acus-walk#expand_home`; pons `log.rs` log directory per platform      | all               |
| `which`   | pons `launcher.rs` (PATH plus user bin dirs, Windows PATHEXT, batch argument refusal)               | pons, velum       |
| `proc`    | acus `cmd_ctx#capture` (process group, timeout, group kill); pons process group, signals, `kill_group`, teardown | all; teardown under `rt` |
| `log`     | pons `log.rs` (directory, retention, levels); acus guard JSONL log                                  | pons, velum, acus |
| `update`  | acus `cmd_update.rs` (GitHub release, target map, SHA-256, atomic replace)                          | all               |
| `claude`  | acus `cmd_skill.rs` (`merge_hooks`, settings backup, skill directories), `agents.rs`                | acus, velum       |
| `out`     | acus `fmt.rs` (`clip`, truncation footers, `--format agent/json/human`)                             | all, optional     |

Also shared: one `install.sh` / `install.ps1` that installs the suite or
single tools, one release workflow with a tool input, and test helpers for
running a built binary (acus `tests/cli/common.rs`, pons `tests/proxy.rs`
`Harness`).

Constraints found:

- Release profiles differ. acus uses `panic = "abort"`, pons needs
  `panic = "unwind"` for `failsafe.rs`. Cargo profiles are per workspace;
  see "Build and toolchain".
- acus startup time matters: `acus guard` runs on every tool call.
  `capsa-core` must stay free of heavy default dependencies; everything
  beyond `std`, `anyhow` and `libc` is feature-gated.
- pons forwards `clientCapabilities.fs` and `terminal` unchanged
  (`src/engine.rs#Engine::route_client`, initialize arm). Phase 2 decides
  whether pons strips them when the agent runs under velum.

Extract a module when a second tool needs it, starting with `which` for
velum, not ahead of time.

### Build and toolchain

Performance comes first; one build setup for every tool.

- `rust-toolchain.toml` pins the current stable (1.99.1 on 2026-10-20) with
  `clippy` and `rustfmt`, so local and CI clippy agree. `rust-version` in
  `[workspace.package]` follows the pin. Bump both together.
- `[workspace.package]`: edition 2024, rust-version, license, repository.
  `[workspace.dependencies]` holds every shared version once.
  `[workspace.lints]` sets the rust and clippy lints for all crates.
- `[profile.release]`: `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`,
  `panic = "abort"`, `strip = true`. Used by acus and velum.
- `[profile.release-unwind]` inherits `release` with `panic = "unwind"`.
  Used by pons until its failsafe no longer needs `catch_unwind`.
- `[profile.dev]`: `debug = "line-tables-only"`; dependencies built with
  `opt-level = 2` so tests on tree-sitter and regex stay fast.
- No `target-cpu=native` in distributed binaries. The default
  `aarch64-apple-darwin` CPU is already `apple-m1`.
- Performance changes need a measurement. acus already has a criterion
  bench, a guard latency bench and the replay corpus (mimalloc was measured
  and rejected: no gain, +20-45% RSS). Candidate after the move: PGO for
  acus in the release workflow, accepted only if the guard and search
  benches improve.
- One CI workflow: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets --all-features -D warnings`, tests on macOS, Linux and
  Windows. One release workflow with a tool input, building that tool's
  binary with its profile for each target, with SHA-256 files.

### Output

Every tool prints three formats from one data model:

- **agent** (default): compact, line-numbered, every truncation names the
  next command. This is acus's current format.
- **human** (`--human`): colour, sections, aligned tables, status symbols,
  wrapped to the terminal width. Colour only on a TTY and never with
  `NO_COLOR`.
- **json** (`--json`): stable fields for scripts.

`capsa-core::out` owns the format flag, the shared palette and the
primitives (section, table, key-value, status line, truncation footer).
Each command builds a result value and renders it through a trait with one
method per format, so the formats cannot drift apart.

`CAPSA_HUMAN=1` makes `--human` the default for a person. It is set only in
the terminals the user types in: Ghostty and Zed's terminal, managed by
MacSet (`ghostty/`, `zed/` terminal env). Agents must never see it, enforced
twice:

1. velum's environment allowlist drops it, so every agent started through
   velum (terminal shim and Zed via pons) runs without it.
2. `capsa-core::out` ignores it when an agent environment is detected
   (`CLAUDECODE=1`, Codex's variables), for agents started outside velum.

`--human` and `--json` on the command line always win; `--agent` forces the
agent format.

After the move: repoint `acus update`, the install scripts, MacSet (Zed's
`claude-pons` entry, README links) and the old repositories' READMEs at
capsa. The installed `acus` keeps working until it updates itself.

## Later backends

- Linux VM via apple/container (1.6.0, macOS 26+): stronger isolation for the
  Linux-capable repositories. Requires a dev image, a long-lived VM with the
  projects mounted at the same path, build caches on a VM volume
  (`CARGO_TARGET_DIR`, pnpm store, `.venv`), and `claude setup-token`.
- macOS VM via Tart: for unattended runs on macOS-native repositories. Heavy
  on a 16 GB / 126 GB machine (about 8 GB RAM per VM, Xcode images of tens of
  GB, at most two macOS guests).

The policy format is backend-neutral so both can be added behind the same
`velum` entry point.

## Phases

1. Prototype: `velum run -- <cmd>` merges policy and env and runs srt.
   Acceptance: the spike table passes from a script, including the Swift and
   Xcode cases without manual flags.
2. Shim and Zed: `claude` shim, `pons -- velum claude-agent-acp`.
   Check whether the ACP adapter executes Bash itself or through Zed's
   terminal capability; if through Zed, pons must strip `terminal` and `fs`
   from `clientCapabilities`.
3. Network prompts through the srt decider, writing grants back.
4. `~/.claude` protection and `request_host_access`.
5. `velum setup` / `doctor`, pnpm install of srt.
6. Move pons in, then acus.

## Open questions

- Can `~/.claude.json` `mcpServers` be protected while the file stays
  writable?
- Does `claude-agent-acp` honour a sandboxed parent, and does it run Bash
  itself or through Zed?
- Which other Keychain items are readable inside the sandbox?
- Seatbelt behaviour on future macOS releases (`sandbox-exec` is deprecated
  but still used by Apple tooling, Codex and srt).

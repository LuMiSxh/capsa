<div align="center">

# Velum

**Claude Code with bypass permissions, inside Anthropic's sandbox runtime.** Velum runs a command, normally `claude`, under `srt` on macOS, with a policy shield around it: a filtered environment, protected config files, a network allowlist and a version floor for Claude Code.

![Platform](https://img.shields.io/badge/platform-macOS-477acc?style=flat-square&labelColor=23252a)
[![MPL-2.0 license](https://img.shields.io/badge/license-MPL--2.0-477acc?style=flat-square&labelColor=23252a)](../LICENSE)

**[Install](#install)** · [Run](#run) · [Policy](#policy) · [Protection](#what-is-protected) · [Commands](#commands) · [Limitations](#limitations) · [Changelog](CHANGELOG.md)

</div>

## Why

The `claude` shim starts Claude Code with `--dangerously-skip-permissions`, so no prompt stands between the agent and the disk. The sandbox is the boundary instead. velum builds the srt settings from a policy, filters the environment, sets `TMPDIR` and only then starts the command. If any step fails, the command does not start, and there is no unsandboxed fallback.

`srt` is Anthropic's [sandbox runtime](https://www.npmjs.com/package/@anthropic-ai/sandbox-runtime), still in beta.

## Install

velum runs on macOS only. The repository is private, so the installer needs a GitHub token (see the [root README](../README.md)):

```sh
curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" \
  https://raw.githubusercontent.com/LuMiSxh/capsa/main/install.sh | sh -s -- velum
```

The binary goes to `~/.local/bin` (`CAPSA_INSTALL_DIR` changes that). Then install srt:

```sh
velum self setup                    # uses pnpm; --package-manager npm or bun
```

`velum self setup` installs srt unless it is already on `PATH` or in a user bin directory, checks that it can be found, and writes the default policy file if none exists. Every entry in that file is commented out, so it changes nothing.

## Run

```sh
velum run -- claude
velum run --dry-run -- claude       # print the srt settings and passed variable names, then exit
```

To make a plain `claude` go through velum:

```sh
velum shim install                  # writes ~/.local/bin/claude (--dir DIR changes the directory)
velum shim status
velum shim uninstall
```

The shim is a POSIX sh script with the absolute paths of velum and of the real `claude` fixed at install time. It runs `velum run -- <claude> --dangerously-skip-permissions "$@"`. Install refuses to overwrite a file velum did not write. Keep the shim directory ahead of the real `claude` on `PATH`: `velum self doctor` checks that.

`velum run` also refuses a program named `claude` older than 2.1.163, with a hint to run `claude update`.

## Policy

The policy file is `~/.config/velum/policy.toml`; `velum run --policy PATH` reads another file. It has two layers: the built-in defaults, then the file. The file extends the defaults and never removes them. Paths may start with `~/`.

```toml
[fs]
write      = ["~/Documents/projects"]          # writable, in addition to the defaults
deny_read  = ["~/Library/Mail"]
deny_write = ["~/Documents/projects/shared"]

[env]
pass        = ["CARGO_*"]                      # exact names, or a trailing * for a prefix
pass_secret = ["ANTHROPIC_API_KEY"]            # exact names only

[net]
allow = ["github.com"]
```

The built-in defaults are:

| Section | Default |
| --- | --- |
| `fs.write` | the working directory, `/private/tmp`, `/private/var/folders`, `~/.claude`, `~/.claude.json`, `~/.cargo`, `~/Library/Caches`, `~/Library/Developer`, `~/Library/org.swift.swiftpm` |
| `fs.deny_read` | `~/.ssh`, `~/.aws`, `~/.config/secrets.env` |
| `fs.deny_write` | the Claude config files and folders in `~/.claude` (settings, `CLAUDE.md`, commands, agents, skills, plugins, output styles, hooks, `local`), `~/.cargo/bin` and cargo config, velum's own files, and the project files listed under [What is protected](#what-is-protected) |
| `env.pass` | `PATH`, `HOME`, `USER`, `LANG`, `TERM`, `COLORTERM`, `SHELL`, and the exact names `CARGO_HOME`, `CARGO_TARGET_DIR`, `CARGO_TERM_COLOR`, `CARGO_INCREMENTAL`, `CARGO_BUILD_JOBS`, `RUSTUP_HOME`, `RUSTUP_TOOLCHAIN` |
| `net.allow` | `api.anthropic.com`, `*.anthropic.com`, `claude.ai`, `platform.claude.com` |

Names that contain `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD` or `CREDENTIAL`, or end in `_KEY`, are dropped even when `env.pass` lists them. Only `env.pass_secret` passes them, by exact name. Keep the policy in your home directory: a policy file inside a repository is writable by the agent.

## What is protected

- **Home secrets.** `~/.ssh`, `~/.aws` and `~/.config/secrets.env` cannot be read. velum refuses a working directory that is `/`, `$HOME`, an ancestor of `$HOME`, or inside `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config`, `~/.local`, `~/Library`, `~/.claude`, `~/.cargo` or `~/.rustup`.
- **Claude config.** The settings, hooks, commands, agents, skills, plugins and `CLAUDE.md` under `~/.claude` are read-only inside the run. `~/.claude.json` stays writable, but velum restores the MCP server and trust keys that changed during the run and warns with their names. Accept trust dialogs and add MCP servers outside velum.
- **Project config.** In the working directory and below, `.claude`, `.mcp.json`, `.gitconfig`, `.vscode`, `.idea`, `.zed`, `.zshrc`, `.zshenv`, `.zprofile`, `.zlogin`, `.bashrc`, `.bash_profile` and `.profile` are read-only.
- **Git.** The hooks and config of every repository (and every bare-repository-shaped directory) under the working directory are read-only, and new repositories stay writable. After the run, velum moves new hook files (other than `*.sample`) to `~/Library/Application Support/velum/quarantine/`, removes `core.hooksPath` and `core.fsmonitor` from the config of new repositories, and warns about other config that runs a program, such as `alias.*` starting with `!`, `core.sshCommand`, `credential.helper` and `filter.*`. A `.git` file that points outside the working directory or through a link is reported and not followed.
- **Environment.** Only the allowlist passes, and secret-looking names are dropped.
- **Network.** Outbound access is limited to the allowlist. Local binding is off, and the metadata address `169.254.169.254` and loopback are unreachable from the sandbox.
- **Claude Code version.** `velum run` refuses a `claude` below 2.1.163 (CVE-2026-55607).
- **Run files.** The generated settings and wrappers live in a private per-run directory under `~/Library/Application Support/velum/run/`, removed after the run.

## Commands

Global flags: `--agent` (default), `--human` and `--json`.

| Command | What it does |
| --- | --- |
| `velum run [--policy PATH] [--dry-run] -- CMD` | Runs `CMD` under srt with the merged policy and filtered environment. |
| `velum self setup [--package-manager pnpm\|npm\|bun] [--dry-run]` | Installs srt if it is missing and writes the default policy file. |
| `velum self doctor` | Checks srt and its version, the policy file, the generated settings, the first `claude` on `PATH`, the shim, its place on `PATH` and `TMPDIR`. Exits 1 when a check fails; a warning exits 0. |
| `velum shim install\|uninstall\|status [--dir DIR]` | Writes, removes or inspects the `claude` shim in `DIR` (default `~/.local/bin`). Install and uninstall touch only files velum wrote. |
| `velum self update`, `velum self uninstall` | Update or remove velum itself. |

## Limitations

- macOS only. srt runs the command with Seatbelt.
- srt is in beta. velum was tested with srt 0.0.80; `velum self setup` installs the version the package manager provides.
- Inside the sandbox, `open` and Apple Events do not work.
- `cargo new` and `cargo init` create no git repository inside velum. Run `git init` yourself.
- Seatbelt cannot nest. `swift` and `xcodebuild` run through velum's wrappers, which add `--disable-sandbox` to `swift` and `-IDEPackageSupportDisableManifestSandbox=1` to `xcodebuild`. Other tools that call `sandbox-exec` need a wrapper of their own.

## Development

```sh
cargo test -p velum                 # unit tests and the shim round trip
sh velum/tests/acceptance.sh        # checks against the real srt; exits 77 when srt is missing
```

## License

MPL-2.0. See [LICENSE](../LICENSE).

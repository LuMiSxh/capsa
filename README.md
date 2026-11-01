<div align="center">

# Capsa

**Tools for coding agents, in one repository.** Each tool has its own version, tag and release. This repository holds the shared CI, the release workflow and the installers.

[![CI](https://img.shields.io/github/actions/workflow/status/LuMiSxh/capsa/ci.yml?branch=main&style=flat-square&labelColor=23252a&color=477acc&label=CI)](https://github.com/LuMiSxh/capsa/actions/workflows/ci.yml)
[![Rust 1.99+](https://img.shields.io/badge/Rust-1.99%2B-477acc?style=flat-square&labelColor=23252a)](rust-toolchain.toml)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-477acc?style=flat-square&labelColor=23252a)
[![MPL-2.0 license](https://img.shields.io/badge/license-MPL--2.0-477acc?style=flat-square&labelColor=23252a)](LICENSE)

**[Tools](#tools)** · [Install](#install) · [Build from source](#build-from-source) · [Releases](https://github.com/LuMiSxh/capsa/releases)

</div>

## Tools

| Tool | What it does | Docs |
|---|---|---|
| **acus** | Code search, outline and editing for coding agents. | [README](acus/README.md) |
| **pons** | Transparent stdio proxy between Zed and the Claude ACP adapter. | [README](pons/README.md) |
| **velum** | Runs Claude Code inside Anthropic's sandbox runtime. macOS only for now. | [README](velum/README.md) |
| **census** | Usage analytics for Claude Code and Codex transcripts. | [README](census/README.md) |

## Install

```sh
./install.sh              # all tools (velum only on macOS)
./install.sh acus pons    # selected tools
```

```powershell
.\install.ps1             # acus, pons and census (velum is macOS only)
.\install.ps1 acus        # selected tools
```

Each tool installs from the newest release tagged `<tool>-v<version>`. The binaries go to `~/.local/bin` (`%USERPROFILE%\.local\bin` on Windows); set `CAPSA_INSTALL_DIR` to change that. `CAPSA_INSTALL_SCOPE=system` adds the directory to the system-wide PATH instead of the user PATH. The installer runs `acus self skill --install` after installing acus; set `CAPSA_NO_SKILL=1` to skip it.

While the repository is private, the installers need a GitHub token. They use `GITHUB_TOKEN` if it is set, otherwise the token from `gh auth token`, so run `gh auth login` first if you use gh.

Uninstall with `./install.sh --uninstall <tool|all>` or `.\install.ps1 -Uninstall <tool|all>`. It removes each binary from the directory the installer uses, and the acus skill unless `CAPSA_NO_SKILL` is set; velum runs `velum shim uninstall` first.

Management commands about the tool itself sit under `self`: `acus self update|skill|uninstall`, `pons self doctor|setup|update|uninstall`, `velum self doctor|setup|update|uninstall`. `velum run` and `velum shim` stay at the top level.

## Build from source

```sh
cargo install --path acus/crates/acus-cli --features cmd-decide
cargo install --path census
cargo install --path pons --profile release-unwind
cargo install --path velum
```

The toolchain is pinned in `rust-toolchain.toml`. pons uses the `release-unwind` profile, which keeps panics unwinding for its failsafe.

## Releases

Each tool has its own version in its `Cargo.toml` and its own section in its `CHANGELOG.md`. The `release` workflow takes a `tool` and a `version`, builds the binaries and publishes the GitHub release `<tool>-v<version>` with the notes from that changelog section.

## License

MPL-2.0. See [LICENSE](LICENSE).

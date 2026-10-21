# Agent Instructions

## Project

capsa is a Rust monorepo of command-line tools for coding agents. Each tool
has its own binary, version, CHANGELOG and AGENTS.md; read the tool's
AGENTS.md before working in its directory.

| Directory | Tool | Purpose |
|-----------|------|---------|
| `acus/`   | `acus`  | Code search, outline, patch, build output (`ctx`) and the PreToolUse guard |
| `pons/`   | `pons`  | ACP proxy between Zed and the Claude ACP adapter |
| `velum/`  | `velum` | Policy shield that runs Claude Code inside Anthropic's sandbox-runtime (`srt`) |
| `core/`   | `capsa-core` | Shared library: hints, output formats, exit rendering, executable lookup, package managers, self update and uninstall (feature `update`) |

`PLAN.md` holds the design, the open questions and the phases.

## Rules

- **Dependencies point to `core` only.** A tool never depends on another
  tool's crates. `core` depends on no workspace crate.
- **Shared code moves to `core` when a second tool needs it**, not earlier.
  `core` has no heavy default dependencies; tokio and similar sit behind
  features, so `acus` stays synchronous and fast to start.
- **CLI shape, every tool:** commands are `<tool> <action>`; actions about the
  tool itself are `<tool> self <action>`. `self` holds installation and
  health: `update`, `uninstall`, `doctor`, `setup` and, for acus, `skill`. A
  rename is hard, with no aliases.
- **Output contract, every tool:** the agent format is the default; `--human`
  gives coloured, structured output (TTY only, never with `NO_COLOR`);
  `--json` is stable. All three render from one result value. `CAPSA_HUMAN=1`
  makes `--human` the default but is ignored under `CLAUDECODE=1` or Codex,
  and velum never passes it to an agent. Exit codes: 0 result, 1 nothing
  found, 2 error with an `error:` line and, when there is a fix, a `hint:`
  line. Every truncation names the command that shows the rest.
- **Performance first, with numbers.** A change made for speed or size states
  the benchmark and the before/after result in the tool's CHANGELOG entry.
  `acus guard` runs on every agent tool call: keep its startup path free of
  new work.
- **Security-sensitive code:** velum's policy defaults, environment filter,
  shim and generated srt settings, and pons's handling of
  `clientCapabilities`. Do not loosen a default without the user's explicit
  approval; every change there needs tests and a review before it is
  committed.
- **Versions:** each tool versions itself (`version` in its own Cargo.toml)
  and records user-visible changes under `## [Unreleased]` in its own
  CHANGELOG. Never bump or release a tool you did not change. Tags are
  `<tool>-vX.Y.Z`.
- **Commits:** prefix the subject with the area: `acus:`, `pons:`, `velum:`,
  `core:`, `ci:`, `docs:`. One area per commit where possible.

## Rust

- **File size (soft):** at most 500 lines of code per file, or 750 with its
  unit tests included. A file over the limit is split when you add to it.
  Longer tests move to a `tests.rs` beside the module
  (`#[cfg(test)] mod tests;`) or to `tests/`.
- **Function size (soft):** at most 100 lines. Split a longer function along
  its steps, not into helpers called once with six parameters. A large
  `match` that reads as one table may stay whole.
- **Modules:** multi-file modules use `foo/mod.rs`, enforced by
  `clippy::self_named_module_files`. No `utils` or `helpers` modules; name
  modules after what they do.
- **Clippy:** `all` plus a chosen set of `pedantic` and `restriction` lints
  in `[workspace.lints.clippy]`. Silence a lint only with
  `#[expect(clippy::…, reason = "…")]`, never with a bare `allow`.
- **Errors:** library crates (`capsa-core`, `acus-*` except `acus-cli`,
  velum's policy code) define `thiserror` enums that callers can match on.
  Binaries collect errors with `anyhow` and `.context(...)`. A fix for the
  user comes from `capsa_core::Hint`, never as text glued into a message.
  `capsa_core::exit` renders `error:`, the context chain and `hint:` in every
  output format.
- **Panics:** `unwrap` and `expect` are refused by clippy outside tests
  (`clippy.toml` allows them in tests). Where an invariant really holds,
  use `#[expect(clippy::unwrap_used, reason = "<the invariant>")]`.
  Release builds use `panic = "abort"`; a tool gets
  `release-unwind` only if it catches panics on purpose to keep running
  (today only pons).
- **unsafe:** only for FFI such as `libc` signals, each block with a
  `// SAFETY:` comment, kept in one module per crate.
- **Dependencies:** prefer `std`. A new dependency names its reason
  (size, build time, maintenance) in the commit message, uses
  `default-features = false` where possible, and goes into
  `[workspace.dependencies]`.
- **Hot paths:** borrow instead of clone, `&str`/`Cow` over `String`, no
  async in code that does not wait on I/O. Measure before and after.
- **Output:** libraries never print; they return values or errors.

## Comments

Code should read without comments. A comment carries what the code cannot
say: intent, a reason, an invariant, an outside constraint.

- **Header:** every `.rs` file starts with
  `// SPDX-License-Identifier: MPL-2.0`. Module and crate files follow it with
  a `//!` line or two saying what the module owns, not how
  (`//! Resolves executables on PATH and in user bin directories.`).
  Test-only files carry just the SPDX line.
- **Doc comments (`///`):** on items used outside their module when the name
  does not state the whole behavior. First sentence in the present tense,
  verb first, ending with a period ("Returns…", "Owns…", "Keeps…"). Then,
  only if they exist: errors, edge cases, ordering or platform notes. Never
  restate the name ("Creates a new `Policy`.").
- **Inline (`//`):** only for a non-obvious why, a workaround (name the
  upstream cause), or a security, concurrency or ordering invariant. Whole
  sentences with a capital and a period, code in backticks, on the line(s)
  above the code. A trailing comment only for a short note in a table-like
  `match` or constant list.
- **Markers:** `// SAFETY:` before every `unsafe` block. `// ponytail:` for a
  deliberate shortcut, saying how to upgrade it. No `TODO`, `FIXME` or `XXX`;
  open work goes in the tool's backlog.
- **Lint expectations:** the `reason = "…"` in `#[expect]` is the comment; do
  not add another above it.
- **Tests:** the test name states the behavior
  (`refuses_cwd_inside_ssh_dir`). Comment only a setup whose purpose is not
  obvious.
- **Never:** commented-out code, section banners (`// ---`, `// ==== X ====`,
  `// MARK:`), history ("previously", "now", "fixed"), notes addressed to a
  reviewer or agent, or a comment that repeats the next line.
- Lines wrap at 100 columns. Shell scripts and TOML follow the same rules
  with `#`.

## Commands

- Run builds and tests through `acus ctx 'COMMAND'`.
- Full check, as in CI: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace --all-features`.
- One tool: add `-p <crate>` (`-p pons`, `-p velum`, `-p acus-cli` and the
  other `acus-*` crates).
- Release builds: `cargo build --release -p acus-cli` (also velum);
  pons only with `--profile release-unwind`.

## Conventions

- Rust edition 2024 on the newest stable, pinned in `rust-toolchain.toml`;
  `rust-version` in the root `Cargo.toml` matches it. New language and
  library features are welcome. Bump both together.
- Versions of shared dependencies live once in `[workspace.dependencies]`;
  crates add features locally. Lints come from `[workspace.lints]`.
- macOS is the primary platform; Linux and Windows are checked in CI. velum
  is macOS-only for now.

## Gotchas

- One `target/` for the whole workspace. Profiles live only in the root
  `Cargo.toml`; a `[profile]` in a member is ignored.
- `release` uses `panic = "abort"`; pons needs `release-unwind` because its
  fail-safe catches panics.
- `serde_json` is built with `preserve_order` for the whole workspace, so
  JSON keeps insertion order everywhere.
- `../Acus` and `../pons` are the frozen former repositories. Work only here.

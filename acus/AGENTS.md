# acus agent instructions

Suite-wide rules are in ../AGENTS.md. This file covers acus only.

## Purpose

acus is a Rust CLI for coding agents. It searches code (`find`, `files`), reads symbols and line ranges (`outline`, `show`, `map`), applies patches (`patch`), runs builds and tests with the noise removed (`ctx`), summarises git (`diff`, `log`), and runs a PreToolUse hook (`guard`) that rewrites or refuses grep, cat and sed calls. The agent skill lives in `skill/` and is embedded in the binary with `include_str!`.

## Layout

Paths are relative to `acus/`. Dependencies point down: `acus-cli` uses every other crate; `acus-search` and `acus-edit` use `acus-syntax` and `acus-walk`; `acus-usage` uses `acus-walk`. `acus-syntax`, `acus-walk` and `acus-decide` use no workspace crate.

- `crates/acus-cli`: the `acus` binary. One `src/cmd_<name>.rs` per subcommand; `src/cmd_ctx/` is a directory module. `src/cmd_guard.rs` holds the guard hook protocol. `src/cmd_self.rs` is the `acus self` group (`update`, `skill`, `uninstall`): `cmd_update.rs` and `cmd_uninstall.rs` call `capsa_core::update` with `TOOL` from `main.rs`; `cmd_skill.rs` prints, installs and removes the skill. Integration tests are in `tests/cli/`.
- `crates/acus-guard`: the guard rules (`src/read/`, `src/search/`, `src/git.rs`, ...) and the rule table in `src/rules/mod.rs`. Parsers are Cargo features (`bash`, `zsh`, `powershell`), enabled from `acus-cli` as `guard-bash`, `guard-zsh` and `guard-powershell` (all default). `src/ast/mod.rs` `choose` picks the backend at run time; without a grammar the guard falls back to `src/ast/legacy.rs`.
- `crates/acus-syntax`: tree-sitter outlines and symbol addresses. `src/lang.rs` holds `Lang::ALL`; `src/rules/mod.rs` (and `src/rules/markup.rs`) maps syntax nodes to symbols.
- `crates/acus-edit`: patch parsing (`src/parse.rs`) and atomic application (`src/apply/`).
- `crates/acus-search`: search with enclosing-symbol context, on the ripgrep crates.
- `crates/acus-walk`: parallel file walking that respects `.gitignore`, and globs.
- `crates/acus-usage`: transcript statistics for `acus usage`.
- `crates/acus-decide`: client for Jev-compatible decision APIs (feature `cmd-decide`).
- `skill/`: `CORE.md` (the short skill) and `SKILL.md` (the full one).
- `docs/guard.md`: the guard rules, listed by name.

## Commands

- `cargo test -p acus-cli`: CLI tests and snapshots. `cargo test -p acus-guard`: guard tests.
- `cargo bench -p acus-cli`: Criterion benchmarks in `crates/acus-cli/benches/core.rs`. Local only.
- `cargo insta review`: accept or reject changed snapshots (needs `cargo-insta`).

## Recipes

- **New subcommand.** Add `crates/acus-cli/src/cmd_<name>.rs` with an `Args` struct and a `run`. Register it in `crates/acus-cli/src/main.rs`: a `mod` line, a `Cmd` variant, and arms in `Cmd::name` and `execute`. Add a snapshot test in `crates/acus-cli/tests/cli/`, a README Commands row and a CHANGELOG entry.
- **New language.** In this order:
  1. A `Lang::ALL` row in `crates/acus-syntax/src/lang.rs` (variant, name, extensions).
  2. A `Lang::grammar` arm behind a Cargo feature. Reuse a feature when the grammar crate is
     shared (`lang-ts` covers TypeScript, Tsx and JavaScript). A new feature goes into the
     feature list and `default` of `crates/acus-syntax/Cargo.toml` and is forwarded in the same
     two places of `crates/acus-cli/Cargo.toml`. `every_listed_language_has_a_grammar` fails
     when the arm is missing.
  3. The variant in `classify` (`crates/acus-syntax/src/rules/mod.rs`; no wildcard, so the
     compiler asks), and any new kind in `KINDS`.
  4. Only if its doc comments or decorators use a prefix `attached_start` (`outline.rs`) does not
     know: add the prefix, or a `#`-comment extension to its `hash` list.
  5. An outline test in `crates/acus-syntax/tests/outline.rs`, gated by the same feature.
  6. The README sentence that lists the languages with symbols (Commands section).
- **New patch directive.** Add a row to the `DIRECTIVES` table in `crates/acus-edit/src/parse/directives.rs`, an `Op` variant in `enum Op` in `crates/acus-edit/src/parse/mod.rs`, a `Kind` arm in `start_op` in `crates/acus-edit/src/parse/ops.rs`, an `apply_op` arm in `crates/acus-edit/src/apply/ops.rs`, and a line in `SYNTAX` in `crates/acus-cli/src/cmd_patch/syntax.rs`. Then update the README Patches section and `skill/SKILL.md`.
- **New guard rule.** Write the rule function in `crates/acus-guard/src/<area>` (`read/`, `search/`, `git.rs`, ...). Add a `Rule` variant in `crates/acus-guard/src/rules/mod.rs`, with its `as_str` name, its `Rule::ALL` entry and the array length. Put tests in `crates/acus-guard/src/tests/`, and list the rule in `docs/guard.md`.

## Contracts

- `--json` output is stable, and output is snapshot-tested with insta. Changing either needs `cargo insta review`.
- Guard rule names (`Rule::as_str`) are logged in `guard.jsonl` and read by `acus usage --guard`. Do not rename one.
- `skill/CORE.md` must stay under 250 words (test `core_stays_short_and_points_to_the_full_skill`). `skill/CORE.md` and `skill/SKILL.md` are embedded with `include_str!` in `crates/acus-cli/src/cmd_skill.rs`.
- Output paths use `/`, also on Windows.

## Gotchas

- `scripts/guard-corpus.sh` regenerates `crates/acus-guard/src/tests/corpus.jsonl` from local guard logs. Its output can contain commands from other private projects. Review and anonymise it before committing; never regenerate blindly. The script refuses to run without its private exclude list, which is no substitute for that review.
- `docs/superpowers/` and `bench-local/` are ignored local notes (root `.gitignore`). Do not commit them.
- Snapshot names come from `snapshot_settings` in `crates/acus-cli/tests/cli/common.rs`: snapshots go to `crates/acus-cli/tests/snapshots/` and are named `cli__<name>`, without a module prefix.
- Guard tests that read Bash need the `bash` or `zsh` feature of `acus-guard`.
- The guard hook is registered in `~/.claude/settings.json` by `acus self skill --install --hooks`, not in this repo, so it also applies to work here.
- CI is suite-level and being rebuilt. The old acus workflows are parked in `.github/legacy/acus/workflows/` at the monorepo root and do not run from here.

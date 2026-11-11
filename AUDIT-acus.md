# Audit: acus

Read-only audit from 2026-11-11. Line counts are raw file lines including tests. Work order and
owners live in [AUDIT.md](AUDIT.md); the step ids below point there.

## acus-cli

About 8.9k lines in `src`, 5.0k in `tests/cli`. All lint suppressions use `#[expect]`.

| # | Finding | Evidence | Step |
|---|---------|----------|------|
| A1 | `cmd_hook.rs::answer` mixes stdin protocol, guard decision, log append and three hand-built `hookSpecificOutput` JSON shapes; the Codex check is inline. Runs on every agent tool call. | `cmd_hook.rs:78-136`, `:87`, `:96-97` clones | 3d |
| A2 | Eight `run` functions carry the same `#[expect(clippy::needless_pass_by_value, reason = "execute hands over the parsed arguments")]`; points to one command signature or trait. | `cmd_blame.rs:39`, `cmd_ctx/mod.rs:190`, `cmd_diff/mod.rs:85`, `cmd_follow.rs:43`, `cmd_hook.rs:34`, `cmd_log.rs:44`, `cmd_patch/mod.rs:68`, `cmd_run.rs:46` | 3d |
| A3 | About 25 `Hinted::new(...)` sites repeat the same shape. | `cmd_follow.rs:257-299`, `source.rs:54-167`, `config.rs:98,114`, `cmd_find/mod.rs:179,212,409,417` | 3d |
| A4 | Several output layers: `outln!`/`outprint!`, `fmt::Out`, `fmt::Budget`, `cmd_show::Sink`, and scattered `eprintln!` notes; the "no matches" and "searched" notes are duplicated. | `main.rs:5-17`, `fmt.rs:86-184`, `cmd_show.rs:65-111`, `cmd_files.rs:89,94`, `cmd_find/mod.rs:265,308,313,316` | 5a |
| A5 | `fmt.rs` has its own colour helpers instead of `render::paint`. | `fmt.rs:48-58`, `core/src/render/paint.rs:42,50` | 5a |
| A6 | Over the soft limits: `cmd_patch/mod.rs` 527 lines without tests, `cmd_map.rs` about 518 code lines, `Shown::show` about 103 lines. | `cmd_show.rs:139-241` | 5a |
| A7 | `cmd_doctor.rs` repeats the log rotation size as a local constant. | `cmd_doctor.rs:24,305-323` | 5a |

## acus-guard

Well structured: nothing over the limits, no printing, `guard.jsonl` contract isolated in
`log.rs`. Uses `capsa_core::log` correctly.

| # | Finding | Evidence | Step |
|---|---------|----------|------|
| G1 | Bash/zsh and PowerShell climb the tree with two copies of the same context logic (`Out`, `Nest`, stdin, gates). | `ast/bash.rs:236-304`, `ast/powershell.rs:146-199` | 4d |
| G2 | `legacy.rs` branches on `ps: bool` throughout and clones the whole command and every segment. `Segment` owns both `low` and `orig`. | `legacy.rs:165-230`, `ast/mod.rs:72-92`, `lib.rs:160` | 4d |
| G3 | Rule names live three times: enum, `as_str`, test-only `ALL` with a manual length. | `rules/mod.rs:14-82` | 2d |
| G4 | Tilde handling duplicates `capsa_core::paths::expand_home`. | `scope.rs:149-161`, `shell.rs:188` | 2d |
| G5 | `log::append` errors are discarded with `let _ =`. | `log.rs:32` | 3d |
| G6 | `prune_if_due` stats a marker on every hint check, before the lock, even when no hint is due. | `hints.rs:119,182` | 2d |
| G7 | Stringly typed: `Verdict::kind() -> &str`, heads matched against string lists. | `lib.rs:110-138`, `search/mod.rs:134` | 4d |
| G8 | Hint lock and state file (nonce, stale break, prune) resemble census `hook/state.rs`; a core candidate if the match holds. | `hints.rs:26-105` | 6 |

## Library crates

| # | Finding | Evidence | Step |
|---|---------|----------|------|
| L1 | `find_range` rebuilds the matcher per address; the whole file is copied into a `String` per hit file; `context_rows` re-collects lines per call; `text.to_owned()` per hit line. | `acus-search/src/lib.rs:112,186,194,298,307-310`, `acus-cli/src/cmd_find/input.rs:30-32` | 2c |
| L2 | `acus-search::follow` is call-graph code used only by `cmd_follow`. | `acus-search/src/follow*` | 2c |
| L3 | `acus-walk` prints a warning from a library. | `acus-walk/src/lib.rs:84` | 2c |
| L4 | `WalkError::GlobSet` overlaps `InvalidGlob`. | `acus-walk/src/lib.rs:22` | 2c |
| L5 | Hand-written regex `escape` while `regex::escape` exists. | `acus-search/src/lib.rs:322` | 2c |
| L6 | `apply`, `apply_with(…, check, allow_broken)` and `apply_with_options` duplicate `ApplyOptions`; `apply_with` has only test callers. | `acus-edit/src/apply/mod.rs:135,148` | 2c |
| L7 | `before` and `lines` cloned on copy and move paths. | `acus-edit/src/apply/files.rs:40-62` | 2c |
| L8 | `acus-edit/src/error.rs` 510 lines; `move_symbol` 101 lines. | `apply/symbols.rs:84-184` | 2c |
| L9 | `Symbol.kind: &'static str` checked only against `KINDS`; `Lang::name` scans `Lang::ALL` with an `expect`. | `acus-syntax/src/outline.rs:13`, `lang.rs:72-79` | 2c |
| L10 | Library result types derive `Serialize` for output. | `outline.rs:11,31`, `acus-search/src/lib.rs:69,82` | 2c |
| L11 | `tempfile` listed under `[dependencies]` of acus-edit; usage in `src` unverified. | `acus-edit/Cargo.toml:19` | 2c |

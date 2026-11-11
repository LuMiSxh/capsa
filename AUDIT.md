# Audit work plan

Unified plan for the internal rewrite from the 2026-11-11 audit. Findings with evidence live in
the area files: [core](AUDIT-core.md), [acus](AUDIT-acus.md), [census](AUDIT-census.md),
[pons](AUDIT-pons.md), [vicarius](AUDIT-vicarius.md). Finding ids (C1, A1, G1, L1, S1, P1, V1)
point there.

Branch: `refactor/workspace-audit-cleanup`. Nothing is committed or pushed until step 8.

## Reading the plan

- Numbers run in order: step 3 starts only when everything it waits for is done.
- Letters run in parallel: 2a to 2d work at the same time, on disjoint files.
- At most 4 editing agents at once. A step can start early when its own "after" list is done
  and a slot is free.

## Order

| Step | Owner | Scope (files) | Findings | After | Why this order and owner |
|------|-------|---------------|----------|-------|--------------------------|
| 0 | main | `CLAUDE.md`, branch | C13 | — | The anyhow exception is a rule change, not code. |
| 1 | worker | `core/src/**` except `claude_settings`, `hooks`; callers of changed signatures in all tools | C1-C5, C10, C12 | 0 | Every later step builds on `fs::write_atomic`, `log::read -> Result` and `expand_home -> PathBuf`. Signature changes touch callers in every tool, so nothing else may run beside it. Mechanical with a clear check: worker. |
| 2a | worker | `core/src/claude_settings/**`, `core/src/hooks/**` | C1 (settings write), C4 (settings copies), C6-C9 | 1 | census (3a) needs the `statusLine` setter. Additive API only, so no tool files change and 2b-2d stay disjoint. |
| 2b | senior-worker | new `core/src/process/**`, `core/src/lib.rs`, `core/Cargo.toml` | C11 | 1 | Signals and the one `unsafe` block are concurrency- and FFI-sensitive: senior-worker. pons (3b) and vicarius (3c) migrate onto it. No migration in this step. |
| 2c | worker | `acus/crates/acus-{walk,search,syntax,edit}/**`, `acus-cli/src/cmd_follow.rs`, `acus-cli/src/follow/**`, `acus-cli/src/cmd_find/**`, `acus-cli/Cargo.toml` | L1-L11 | 1 | Needs only step 1. Library cleanups with tests and a bench as the check. `follow` moves into acus-cli before 3d reshapes the commands. |
| 2d | worker-ds | `acus/crates/acus-guard/**` | G3, G4, G6 | 1 | Small, well-scoped guard fixes with a clear check; the parser rewrite (4d) comes later on a clean base. |
| 3a | worker | `census/**` | S1-S5, S14 | 1, 2a | Needs `write_atomic` and the `statusLine` setter. One transcript parser first, because 4a splits the modules that use it. The design is settled in the brief: worker. |
| 3b | senior-worker | `pons/src/proxy/**`, `pons/Cargo.toml` | P1-P3 | 2b | Moves signals and process-group teardown onto `capsa_core::process`; shutdown and child killing are concurrency-sensitive. |
| 3c | senior-worker | `vicarius/**` | V1-V4 | 1, 2b | Atomic write, signals, and the check of an `await` held under a lock: concurrency-sensitive. |
| 3d | worker | `acus/crates/acus-cli/src/**` except the output layer, `acus-guard/src/log.rs` | A1-A3, G5 | 2c, 2d | The hook path needs the settled guard API (2d) and the moved `follow` (2c). Typed hook payload, one command signature, a hint constructor. |
| 4a | worker | `census/**` | S6-S13 | 3a | Splits and typing on top of the unified parser. |
| 4b | senior-worker | `pons/src/engine/**`, `pons/src/fold/**` | P4-P6 | 3b | Touches the `clientCapabilities` path: security-sensitive, tests for every change, reviewer before commit, no default loosened. |
| 4c | worker-ds | `vicarius/**` | V5-V7 | 3c | Status client onto hyper and a typed body rewrite; well scoped with existing tests. |
| 4d | senior-worker | `acus/crates/acus-guard/**`, guard call sites in `acus-cli/src/cmd_hook.rs` | G1, G2, G7 | 2d, 3d | Unifies the bash, zsh and PowerShell walkers. The guard is the security boundary of every agent tool call, and the corpus snapshots plus a bench must hold. |
| 5a | worker | `acus/crates/acus-cli/**` | A4-A7 | 4d | One output layer, file splits, `render::paint`. Last in acus, because it touches every command that 3d and 4d changed. |
| 5b | worker-ds | `pons/src/doctor/**`, pons doctor tests and snapshots, `pons/CHANGELOG.md` | P7 | 4b | Removes the legacy JSON output; Luca is the only pons user, so no consumer depends on its bytes. Well scoped, user-visible: CHANGELOG entry. Runs beside 5a on disjoint files. |
| 6 | Explore, then worker | `acus-guard/src/hints.rs`, `census/src/hook/state.rs`, `core` | G8, S15 | 4a, 5a, 5b | Moves the hint and state lock into core only if Explore confirms both are the same mechanism. Otherwise it is closed as "different". |
| 7a | test-runner | workspace | — | 6 | Full CI check over the combined change. |
| 7b | reviewer | whole branch diff | — | 6 | Reviews against this plan's acceptance rules, with the pons `clientCapabilities` path as its focus. Runs beside 7a, read-only. |
| 7c | test-runner | `acus-cli` find bench, HEAD vs branch | L1 | 6 | 2c measured `find TODO` about 12% slower under load (69 → 78 ms) without an A/B. Runs alone on a quiet machine; a real regression goes back to 2c's owner before step 8. |
| 8 | main | — | — | 7a, 7b, 7c | Fixes from 7a and 7b go back to the step's owner. Then one commit per area (`core:`, `acus:`, `census:`, `pons:`, `vicarius:`, `docs:`) and a push of the branch. |

Deferred, no step: C14-C16 (wait for a tool that needs them), V8 (step runner), P8 (JSONC stays
in pons).

## Rules for every agent

- Read `CLAUDE.md`, the area's `AGENTS.md`, this file and the area's `AUDIT-*.md`. Never open
  `*-plan.md`.
- Do not commit, push, create branches or worktrees, or bump versions.
- Edit only the files in your step's scope. Other agents edit other areas at the same time: a
  build error in a file outside your scope goes into your report, not into a fix.
- Run cargo through `acus ctx`, scoped with `-p <crate>`; use `cargo fmt -p <crate>`, not
  `--all`.
- Behaviour stays the same unless the finding says otherwise. Snapshot and corpus tests must
  pass unchanged. A snapshot that changes needs a reason in the report.
- A speed or size change states the benchmark and its before and after numbers in the tool's
  CHANGELOG under `## [Unreleased]`. Other entries only for user-visible changes; core has no
  CHANGELOG.
- Never loosen a pons `clientCapabilities` default.
- Report: changed files, each check command with exit code and output tail, findings left open
  and why.

## Acceptance for the whole branch

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  and `cargo test --workspace --all-features` pass.
- Every finding in a step is fixed or listed as open with a reason in the step's status below.
- No new duplicate of a core helper; no `unsafe` outside `core/src/process` and the existing FFI
  modules.

## Status

| Step | State | Notes |
|------|-------|-------|
| 0 | done | Branch created; anyhow exception written into CLAUDE.md. |
| 1 | done | `core::fs::write_atomic`, `log::read -> Result<_, LogError>`, `expand_home -> PathBuf`, `hooks/state.rs`. C1 partly: `update/binary.rs` keeps its own staging (refuses a planted temp file, sets mode 0o755); the settings write moves to 2a. C5 closed: counting drops needs every line parsed. C12: all four items are used, nothing deleted. census doctor now warns on an unreadable hook log: 3a writes the CHANGELOG entry. |
| 2c | done | `follow` moved to `acus-cli/src/follow/`; `find_ranges` builds the matcher once; `context_rows` one pass; `walk` no longer prints (`missing_roots`, printed by `source::warn_missing_roots`); `WalkError` = `PathNotFound`, `InvalidGlob`; `regex::escape`; one `acus_edit::apply(ops, root, &ApplyOptions)`; `error/` split with `hint.rs`; `move_symbol` split. Closed: L7 (both copies needed), L11 (`tempfile` used). Open by design: L9 `Symbol.kind` enum, L10 `Serialize`, `Hit.text` borrow; each changes `--json` or a CLI filter. Bench unexplained, see 7c. |
| 3d | done | A1: one typed `HookOutput`, wire bytes pinned by `hook_replies_keep_their_wire_bytes`. A2: every `run` takes `&Args`, the eight expects are gone. A3: private `hinted()` in `main.rs`, texts byte-identical. G5: `acus_guard::log::append` returns `io::Result`; the hook still drops it, new doctor check `guard log file` makes it visible (CHANGELOG Added). `ctx` on `core::process` (feature `process`, no `libc`); Windows now kills the whole tree (CHANGELOG Fixed, not run on Windows). 7b: `doctor_fails_when_the_guard_log_is_read_only` fails when run as root. |
| 4d | done | G1: one parent walk in `ast/climb.rs`, each dialect a `Climber` (role table, capture rule, chain hook). G2: `Flavor` trait (`Sh`, `Ps`) replaces `ps: bool` in `legacy.rs`; no whole-command or double segment clone; `Segment.orig` is `None` when equal to `low`; no lowercase allocation for lowercase ASCII. G7: typed `Kind`, `is_grep()`/`is_sink()`. Verdicts, corpus, `guard.jsonl` and hook bytes unchanged. Bench (fixed to `acus hook --replay`): 34.7 → 34.8 µs per decision, noise, no CHANGELOG. Open: `low` stays owned (a lifetime through every rule), rules still take `ps: bool`. Pre-existing bare `allow(dead_code)` under `--no-default-features` goes to 5a. |
| 5a | done | One output module `acus-cli/src/out/` replaces `fmt.rs`, macros and budget code; colour through `render::paint`; `cmd_map/` and `cmd_patch/formatter.rs` split; `Shown::show` split; one rotation constant; guard builds clean under `--no-default-features`. Snapshots unchanged. Open: the shared "(no files…)"/"(searched …)" note helper (low payoff). Main updated `concurrent_guard_calls_count_exactly`: the `.lock` file now stays by design, the test asserts the lock is free. |
| 7a | done | fmt, clippy (workspace, core and guard without default features), 1579 tests, release `acus-cli`, pons `release-unwind`: all exit 0. |
| 7b | done | No correctness findings across all focus areas. Fixed: root guard in `doctor_fails_when_the_guard_log_is_read_only` (main); `census/src/usage/tools.rs` over the file limit, split by worker-ds. Noted: `fs::write_atomic` now reports a failed directory sync after a successful rename; `inline_tools` has no test for duplicate keys or an escaped `type`. |
| 7c | done | Noise, no regression. hyperfine HEAD vs branch: 10.1/10.1, 10.6/10.7, 12.7/12.7 ms; criterion A/B/A/B within ±3% with 10 ms run-to-run swings on the TODO case. The 2c figure did not reproduce. Main also made the directory sync in `fs::write_atomic` best effort (from 7b). |
| 8 | done | One commit per area, branch pushed. |
| 6 | done | Explore: partly the same. census uses OS locks (`File::try_lock`) in `hook/state.rs` and `ledger/state.rs`; acus uses a `create_new` lock file with nonce and a 2 s stale break, kept only because of an outdated MSRV comment. Decision: one `capsa_core::lock` on OS locks for both tools; acus drops nonce and stale break (the kernel releases on exit), bench required. Done: `capsa_core::lock::{exclusive, shared}(path, wait) -> io::Result<Option<Lock>>` (1 ms poll, no truncate). acus hints on it (nonce, stale break and `.brk` gone, old `.lock` files still work, prune removes idle `.lock`); census hook state and ledger on it, hook state write on `fs::write_atomic`. Hint check 247.8 → 147.8 ms per 1000 calls (CHANGELOG Changed). |
| 2a | done | statusLine API (`StatusLineSpec`, `StatusLineState`, `HookFile::{status_line,set_status_line,remove_status_line}`); C4 one `read_text`; C6 restructured, no `unreachable!`; C9 `Shape { path, key, expected }`, `Changed { path, change }`. C7 already held (`HookFile` goes through `claude_settings` IO). C1 rest closed: `claude_settings::write` keeps its own staging so `confirm_unchanged` stays right before the rename. Main then made `fs::write_atomic` keep the target's permissions (test added). |
| 3a | done | One transcript model in `transcript.rs` (`Record`, `records`, `cwd_of`, `list_files`); `usage/claude.rs::parse` split into a `Parser`; one `parse_ts` (now rejects a space separator); statusLine through `HookFile`; census `Step` kept for `Skip`/`ReplaceForeign`; both atomic writes on core (main added the parent-directory sync to `fs::write_atomic`, which the ledger had before). Goldens unchanged. S2: `usage`, `statusline`, `limits` read each file once; `hook` and `session` read twice, moved to 4a. CHANGELOG `### Fixed` for the doctor warning. |
| 4a | done | S6 unwraps restructured; S7 `usage_views/` by view; S8 `Acc::file` split, `merge/` with `agents.rs`; S9 `RULES` table; S10 typed (`cmd_doctor` already via the 3a statusLine API); S11 `UNKNOWN`; S13 `tests/cli/` (222 tests, every line kept). S2 `session` carries cwd. Open: S2 `hook` tail sharing (needs a byte source through `limits::status_by` with its 150 ms deadline, ledger update and scan); S12 `limits/mod.rs` not split (334 lines, shared types, no gain). |
| 2d | done | G3: one `rules!` table. G6: prune only when a hint shows (test added, no bench, no CHANGELOG). G4 partly: `scope::home_dir` delegates to core; `shell::expand_home` stays because it expands `~` inside whole shell words (`--file=~/x`), which core does not. |
| 3c | done | V1: both writes on `core::fs::write_atomic`. V2: `core::process::shutdown_signal`. V3: one stderr line per run of record failures (`AtomicBool`), no test. V4: sound, no change (no std guard across `.await`, poison handled, refresher joined). CHANGELOG `### Fixed` for V1 and V3. |
| 4c | done | V6: `inline_tools.rs` reads typed blocks, keeps the rest as `RawValue` in request order via a custom ordered `Fields` deserializer; untouched parts are forwarded as raw text. 7b reviews that deserializer. V5 closed: `status_client` is sync and runs on every SessionStart; hyper or blocking reqwest would add a runtime. V7 closed: vicarius doctor spawns no `ps`; pons doctor is sync. |
| 2b | done | `capsa_core::process`: feature `process` (sync: `own_group`, `signal_group`, `group_alive`, `exit_code`, the only `unsafe`) and `process-async` (tokio: `Signals`, `shutdown_signal`, `wait_group_gone`). Escalation timing stays with callers. For 3d: on Windows `ctx` will kill the whole tree via `taskkill` instead of only the shell; update the doc comment and the CHANGELOG. |
| 3b | done | pons on `core::process` (`process-async`); no `unsafe` in `pons/src`; `libc` only as unix dev-dependency. `proxy/` split into `mod`, `wiring`, `session`, `signals`, `teardown`. No behaviour change, no CHANGELOG. |
| 4b | done, reviewed | Typed borrowed views `fold::UpdateView`, `engine::view::ResponseView` (hand-built so a mistyped field still reads as absent). `initialize.rs` and capability logic in `wire.rs` untouched; P5 needed no change (unchanged lines already forward raw bytes). P6: `poll` split, clones replaced by moves. New `engine/tests/capability_bytes.rs` pins 14 `initialize` shapes byte for byte. Bench `engine_line_cost` within noise, no speed claim. Reviewer: no findings (lookups equivalent incl. null, non-string and duplicate keys; capability tests would catch a loosened default; fold order unchanged). |
| 5b | done | `legacy_json` deleted; `pons self doctor --json` now emits the shared `render::Doctor` shape (`tool`, `version`, `ok`, `groups`), checked live. Legacy-only field and tests removed; README, AGENTS.md and CHANGELOG `### Removed` updated. |

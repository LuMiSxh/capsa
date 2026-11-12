# Audit round 2: consistency, speed, smarter tools

Read-only audit from 2026-11-11, after round 1 ([AUDIT.md](AUDIT.md)). Four directions: tools
consistent through `capsa-core`, faster hot paths, learning from usage with proactive hints, and
tools that know each other. Hard renames are allowed (Luca is the only user); every format change
gets a CHANGELOG entry.

Decided 2026-11-11: D1 all logs in the tool's state dir, pons included; D2 TOML everywhere; D3 no;
D4 no; D5 later as its own step.

Finding ids: K consistency, T timing, M smarter, X cross-tool. Sizes: S, M, L.

## Decisions needed before work starts

| # | Question | Recommendation |
|---|----------|----------------|
| D1 | Where do logs live? `guard.jsonl` is in acus's config dir, `hook.jsonl` in census's state dir, pons logs in `~/Library/Logs/pons`. (K14) | All logs in the tool's state dir; core owns the path (`paths::log_file(tool, name)`), census reads the guard log through it. pons keeps `~/Library/Logs` only if Console.app matters to you. |
| D2 | One config format? acus uses TOML (hand-edited), census and vicarius JSON (written by `self config set`). (K16) | TOML everywhere through one core `config` module (load, save atomically, `self config show/set`); acus already depends on `toml`. Size L, breaking. |
| D3 | May vicarius route on the Claude subscription forecast (census snapshots)? (X3) | No for now: the DeepSeek gate is about credit, not about Claude limits; mixing them hides why an agent was offered. |
| D4 | May acus change output budgets when census reports a full context? (X5) | No: it adds a read to every tool call and changes agent output that snapshots pin. |
| D5 | pons shows census data in Zed? (X7) | Later, as its own step with a security review; it touches what pons sends to the client. |

## Findings

### Consistency (K)

| # | Finding | Evidence | Size |
|---|---------|----------|------|
| K1 | Doctor helpers (`check`, `writable`, `same_file`, dir and PATH checks) exist four times. | acus `cmd_doctor.rs:40-52,294-301`, census `cmd_doctor.rs:36-89,244-251`, vicarius `cmd_doctor.rs:70-82`, pons `doctor/view.rs:43-55` | S |
| K2 | Log health checks (writable, size against rotation, last entry age) are local to acus and census; vicarius has none for `proxy.jsonl`. | acus `cmd_doctor.rs:305-360`, census `cmd_doctor.rs:254-273` | M |
| K3 | Rotation limits are hard-coded per tool and repeated in doctor text ("rotate at 4 MiB"). | acus-guard `log.rs:9`, acus `cmd_doctor.rs:333`, census `hook/state.rs:13`, `prices/source.rs:15`, vicarius `record.rs:16` | S |
| K4 | census copies acus's `ACUS_CONFIG` logic to find `guard.jsonl`. | census `cmd_usage.rs:330-336`, acus `config.rs:62-66` | S |
| K5 | census `Config::save_to` uses plain `fs::write`. | census `config.rs:155-161` | S |
| K6 | Feature-off stubs for `self update`/`uninstall` are copied in three tools. | acus `cmd_update.rs`, `cmd_uninstall.rs`; census `cmd_self.rs:55-64,96-103,131-137`; vicarius `cmd_self.rs:44-54` | S |
| K7 | `hinted()` is private to acus; census and vicarius build `Hinted::new` by hand. | acus `main.rs:143` | S |
| K8 | Relative time formatted twice in acus with different units. | acus `cmd_doctor.rs:363` (`ago`), `cmd_log.rs:121` (`age`) | S |
| K9 | Truncation footers: census `sessions` names flags, not a command; census `models` and pons doctor notes cut silently. | census `cmd_usage.rs:199-224,272-277`, pons `doctor/mod.rs:33` | S |
| K10 | census keeps its own `Step` beside `claude_settings::Step`. | census `setup.rs:102-110` | S |
| K11 | `census self skill` has no `--target`; acus has `claude|codex|all`. | census `cmd_self.rs:90` | S |
| K12 | `self update` reinstall: acus ignores the child's exit status, census returns an error. | acus `cmd_update.rs:36-51`, census `cmd_self.rs:112-129` | M |
| K13 | The uninstall leftover message is printed by each tool. | census `cmd_self.rs:156-160`, acus `cmd_uninstall.rs:46-51` | S |
| K15 | pons accepts `--json`/`--human` only under `self`. | pons `cli.rs:240-242` | S |
| K17 | `exit::Outcome::Code(u8)` is used only by `acus ctx --json`. | acus `cmd_ctx/print.rs:24` | S |
| K18 | `census usage --guard` prints `hint:` with `eprintln!`, outside `render`/`exit`. | census `cmd_usage.rs:437-442` | S |
| K19 | No doctor reports an available update. | all four doctors | S |
| K20 | acus doctor reads the whole guard log for its last entry. | acus `cmd_doctor.rs:340-345` | S |
| K21 | Doctor hints are `Option<String>` on `Check`, not `capsa_core::Hint`. | `core/src/render/check.rs` | S |

### Timing (T), measured with hyperfine (`-N -w 5 -r 50`, release, p50/p95 ms)

Floor `/usr/bin/true` 0.78. `acus hook` 1.9-2.2 (decision itself about 0.1). `census statusline`
1.6, `census hook` SessionStart 1.7. `census hook` on a prompt: 3.0 with a current digest, 6.5 at
1440 transcript files, **21.4 / 29.7 when the call adds rows** (most calls), 33.8 cold.
`vicarius hook` 3.0. pons engine 0.9-2.3 µs per line.

| # | Finding | Evidence | Size |
|---|---------|----------|------|
| T1 | Every appended ledger row invalidates the digest, so the next read rebuilds it from all rows and rewrites `digest.json`: 3.0 → 21.4 ms on most `census hook` calls, more on the real 2 MB rows file. The updater already holds the new rows and could fold them in. | census `ledger/digest.rs:108-116`, `ledger/mod.rs:171-183` | M |
| T2 | Each `census hook` walks every transcript under `projects/` (1438 files, 1.0 GB here); cost grows linearly. Skip the sweep when the last one is seconds old and only check the current transcript. | census `transcript.rs:167`, `ledger/scan.rs:29` | M |
| T3 | `census_snapshot::read_since(0)` scans the whole snapshot file on each limits call. | `core/src/census_snapshot.rs:96-121` | S |
| T4 | `acus hook` spends about 1.1 ms above the process floor on load and init, ten times the decision. A lean hook path (smaller binary or lazy init) is the only lever; a daemon is not worth it. Needs profiling first. | 23.6 MB binary | L |
| T5 | `vicarius hook` links tokio, hyper, reqwest and rustls but uses none: `--version` 2.55 ms against acus 1.88. | `vicarius/Cargo.toml`, `cmd_hook.rs` | S-M |
| T6 | `vicarius hook` waits up to 300 ms when the daemon accepts but hangs, on every SessionStart. | `vicarius/src/cmd_hook.rs:18` | S |

A daemon for `census statusline` or SessionStart gains nothing: both already run at about 1.6 ms.

### Smarter (M)

| # | Finding | Evidence | Size |
|---|---------|----------|------|
| M1 | Guard hints repeat every `hint_every` calls whether or not the agent follows them; nothing records whether a hint was followed. Derive a per-rule follow rate off the hot path (census over `guard.jsonl`, join on the next call in the session) and let the guard read it in O(1) to go quiet on followed rules and repeat ignored ones (replaces fixed `hint_every`, capped range). | acus `config.rs:49-58`, `hints.rs:30-57`, `log.rs:35-49` | M |
| M2 | `census usage --guard` has no per-project view, so a rule that keeps firing in one repo is invisible. | census `cmd_usage.rs:372-476` | S |
| M3 | Thresholds are fixed constants (context 60/80 %, limits 75/90 %, budget 80/100 %, forecast at 50 %). A rising burn rate is not warned about before the fixed level. Warn once per window when the snapshot burn rate is above the calibrated typical rate. | census `hook/levels.rs:15-40`, `limits/forecast.rs`, `limits/calibrate.rs:12-18` | M |
| M4 | Calibration needs 4 points within 30 days; new installs get no forecast for days. Widen the window while points are few. | census `limits/calibrate.rs:12-18` | S |
| M5 | Doctors report but never repair; every fix is a command the user retypes. `self doctor --fix` runs the safe fixes (missing hook, stale skill, missing dir) and prints what it did. | all doctors | M |
| M6 | `acus ctx` does not notice the same build failing repeatedly in a session. After the third identical failure, lead with the previous failure's fix line. Needs a small per-session state file written after the command. | acus `cmd_ctx` | M |
| M7 | `census usage --handbacks` shows counts per week, not which role handed back after how long. | census `usage/handback.rs:9-10`, `usage_views/handbacks.rs` | S |
| M9 | Proposed, not scheduled: on SubagentStop census reports the returned agent's tokens, cost and duration, so roles are chosen from real numbers. | census hooks | M |
| M10 | Proposed, not scheduled: the status line and warnings say whether the week runs ahead of or behind plan, not only its percentage. | census `hook/levels.rs`, `limits/forecast.rs` | S |
| M11 | Proposed, not scheduled: the SessionStart line names hand-backs per role over 7 days. Needs M7. | census hook, `usage/handback.rs` | S |
| M8 | The vicarius SessionStart line is fixed prose; it could name roles where -ds results were handed back often. Hand-backs are inferred from spawn timing only, so this needs a minimum sample. | vicarius `cmd_hook.rs:70-101` | L |

### Cross-tool (X)

Today only `census_snapshot` carries data between tools (census and pons write, census reads), and
census reads `guard.jsonl` by contract. vicarius and acus know nothing about the others.

| # | Finding | Evidence | Size |
|---|---------|----------|------|
| X1 | No shared place for a tool's current status. Add `capsa_core::bulletin`: one small typed JSON per tool in `state_dir("capsa")/bulletin/<tool>.json`, written atomically off the hot path, every value with a timestamp and TTL; a missing or stale file reads as unknown. | `core/src/paths.rs:8-22` | M |
| X2 | `census statusline` cannot show the DeepSeek balance or gate without a socket call. vicarius writes its gate and balance to the bulletin on each refresh; the statusline reads it. | vicarius `refresh.rs`, census `cmd_statusline.rs:20-42` | S |
| X4 | Guard blocks are visible only in `census usage --guard`. acus writes a per-session count at SessionStart and on hint writes (already off the decision), census shows it in the statusline. | acus `hints.rs`, census `statusline.rs` | S |
| X6 | No doctor knows whether the sibling tools are installed and healthy. Each `self doctor` gets an `interop` group: per sibling, whether it is installed (`which::find`), its bulletin age, and which interop feature uses it, active or off (not installed = info, never a warning). Every tool works alone: an interop feature is off when the sibling is missing, and `self uninstall` removes the tool's own bulletin so a stale one is not read. | all doctors, `self uninstall` | S |
| X8 | Update awareness: each tool records its version and the last known latest release in its bulletin; doctors and the statusline show "update available". Pairs with K19. | `core/src/update` | S |
| X9 | Zed's picker via pons lacks models terminal `claude` has (Haiku 5.5, Fable 5.2): the adapter runs the Claude Code CLI bundled in its SDK (adapter 0.85.1 -> CLI 2.1.286), not the user's `claude`, and the pnpm install stayed at 0.85.1. pons doctor compares the adapter with npm `latest` and the bundled CLI with `claude --version`; the hint is the reinstall command (no `--fix`). OAuth `/v1/models` rejected: against the Claude Code terms, and the adapter refuses ids outside its list. | `pons/src/doctor/adapter.rs`, adapter `acp-agent.js:626-662` | S |
| X10 | DeepSeek models (default `deepseek-flash` "Deepseek Flash", `deepseek-v4-pro` "Deepseek V4 Pro", names in a new pons `config.toml`) as main agent in pons while vicarius is active, off when it is absent; vicarius gains an "is active" check for pons. Open: the adapter adds custom ids only through Claude settings `availableModels` + `modelOverrides` (an allowlist read from `~/.claude` or `CLAUDE_CONFIG_DIR`), which would also restrict terminal `claude` or move the login. Spike again on adapter >= 0.106 before planning. | adapter `session-model.js:272-326`, `acp-agent.js:5448-5493` | M |

## Order

Numbers run in order, letters in parallel on disjoint files, at most 4 editing agents.

Rules for every agent:

- Read `CLAUDE.md`, the area's `AGENTS.md` and this file. Never open `*-plan.md`.
- Do not commit, push, create branches or worktrees, or bump versions.
- Edit only the files in your step's scope. A build error in a file outside your scope goes into
  your report, not into a fix.
- Run cargo through `acus ctx`, scoped with `-p <crate>`; use `cargo fmt -p <crate>`, not `--all`.
- Behaviour stays the same unless the finding says otherwise. Snapshot and corpus tests must pass
  unchanged; a snapshot that changes needs a reason in the report.
- A speed or size change states the benchmark and its before and after numbers in the tool's
  CHANGELOG under `## [Unreleased]`. Other entries only for user-visible changes; core has no
  CHANGELOG.
- Never loosen a pons `clientCapabilities` default.
- Report: changed files, each check command with exit code and output tail, findings left open and
  why.

| Step | Owner | Scope | Findings | After | Why this order and owner |
|------|-------|-------|----------|-------|--------------------------|
| 0 | main | this file | D1-D5 | Luca | Decisions shape step 1. |
| 1a | worker | new `core/src/doctor/` | K1, K2, K3, K21 | 0 | One doctor kit with log checks that take the rotation limit as a value; every tool adopts it in step 2. |
| 1b | worker | `core/src/render`, `core/src/hint`, `core/src/update`, `core/src/paths.rs` | K6, K7, K8, K9 (`Doc::more`), K12, K13, D1 path fn | 0 | Shared output, hint, update and path pieces; disjoint from 1a. |
| 1c | worker | new `core/src/bulletin.rs` | X1, T3 (tail read in `census_snapshot`) | 0 | Format owner for every cross-tool feature; small, typed, tested. |
| 1d | worker | new `core/src/config/` | D2 | 0, only if D2 = TOML | Format change touches census and vicarius; done once in core. |
| 2a | worker | `acus/**` | adopt 1a-1d, K20, D1 log move | 1a-1d | Each tool moves onto the kit in one step. |
| 2b | worker | `census/**` | adopt 1a-1d, K4, K5, K10, K11, K18, D1 | 1a-1d | census also reads the moved guard log through the core path. |
| 2c | worker-ds | `pons/**` except `engine/**` | adopt 1a, 1b, K15 | 1a, 1b | Well scoped; no clientCapabilities path. |
| 2d | worker-ds | `vicarius/**` | adopt 1a-1d, K2 log check, T6 | 1a-1d | Well scoped with existing tests. |
| 3a | senior-worker | `census/src/ledger/**` | T1 | 2b | Incremental digest; correctness of every cost number depends on it. Bench before and after. |
| 3b | worker | `census/src/transcript.rs`, `ledger/scan.rs` | T2 | 3a | Same crate as 3a, after it. Bench. |
| 3c | worker | `vicarius/Cargo.toml`, `cmd_hook.rs` | T5 | 2d | Feature split so the hook path links no async stack. Bench. |
| 3d | Explore, then decision | `acus-cli`, `acus-guard` | T4 | 2a | Profile load and init first; implement only with a measured lever. |
| 4a | worker | `census/src/usage/**`, guard log reader | M1 (follow rate), M2, M7 | 2b | Off-path analysis feeding 4b. |
| 4b | senior-worker | `acus-guard/src/hints.rs`, `acus-cli` config | M1 (adaptive throttle) | 4a, 1c | Reads 4a's output in O(1) on the hot path; bench required. |
| 4c | worker | `census/src/limits/**`, `hook/levels.rs` | M3, M4 | 3a | Uses the faster ledger; warning text in the skill if it changes. |
| 4d | worker | doctors in all tools | M5 | 2a-2d | `--fix` on top of the shared kit. |
| 4e | worker | `acus-cli/src/cmd_ctx/**` | M6 | 2a | New per-session state, written after the command. |
| 5a | worker | vicarius refresh, acus SessionStart, census statusline | X2, X4, X8, K19 | 1c, 2a-2d | Writers and the statusline reader of the bulletin. |
| 5b | worker | doctors | X6 | 5a | Neighbours group reads the bulletins. |
| 5c | worker | `pons/src/doctor/**` | X9 | — | Adapter and bundled CLI staleness checks. |
| 5d | spike, then plan | pons, vicarius | X10 | 5b, 5c | Interop: off when vicarius is absent; doctor `interop` group shows it. |
| 6 | test-runner, reviewer, benches | workspace | — | all | Full check, review against this plan, benches for T1-T5 and 4b, skill evals where SKILL.md changed. |

Deferred: M8 (needs better hand-back data), D3/D4 if answered no, D5 (own step with security
review), K17 (keep `Code` for `ctx`), K21 (`Hint` is a trait; `Check` already keeps its hint as a
separate field), C14-C16 from round 1.

T4 and T5, measured 2026-11-12 and dropped:

- T4: `acus hook` 2.21 ms p50 = process floor 0.74 + Rust exec and std init ~0.5 + acus startup
  (clap, paging) ~0.55 + hook input ~0.16 + decision, reply and log ~0.22. Levers: a separate
  `acus-hook` binary ~0.4-0.5 ms, dispatch before clap ~0.1-0.3 ms. Not worth a change.
- T5: `vicarius --version` 3.33 ms against acus 2.20 ms; ~1.0-1.3 ms is dyld loading
  Security and CoreFoundation, pulled in only by `rustls-platform-verifier` through reqwest's
  `rustls` feature, with no feature to drop them. The only lever is a second `vicarius-hook`
  binary, for about 1 ms once per session. Not worth it.

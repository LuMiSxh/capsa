# Audit: census

Read-only audit from 2026-11-11. Line counts are raw file lines including tests. Work order and
owners live in [AUDIT.md](AUDIT.md); the step ids below point there.

## State

About 9.5k lines in `src` over about 70 files, 3.5k in tests. `render`, `exit`, `hint`, `log`,
`paths`, `skill`, `update`, `hooks` and `census_snapshot` are used as intended. No bare `allow`.

## Findings

| # | Finding | Evidence | Step |
|---|---------|----------|------|
| S1 | Three transcript parsers with duplicated serde structs and the same "assistant, not `<synthetic>`, has usage" filter. `transcript.rs` already offers `records()` and `Record::request()`, the other two do not use them. | `transcript.rs:14-101`, `usage/claude.rs:20-90,128-228`, `ledger/parse.rs:8-66` | 3a |
| S2 | Transcripts are opened by several readers per call; whether the hook path reads one file twice is unverified. | `session.rs:292-310`, `context.rs:152-193`, `usage/read.rs:18-49`, `ledger/scan.rs`, `ledger/update.rs` | 3a |
| S3 | Two ISO timestamp parsers. | `ledger/parse.rs:85-99`, `usage/time.rs:9-32` | 3a |
| S4 | `statusLine` wiring edits `settings.json` through raw `Value` with its own `Step` enum, while hooks go through `capsa_core::hooks`. | `setup.rs:60-101,164-178` | 3a |
| S5 | Atomic write implemented locally twice. | `ledger/state.rs:93`, `usage/prices/source.rs:121` | 3a |
| S6 | `unwrap()` in non-test code without `#[expect]`. | `usage/tools.rs:156,163` | 4a |
| S7 | `usage_views.rs` 562 lines with no tests, over the 500 limit. | `usage_views.rs` | 4a |
| S8 | Functions over 100 lines: `Acc::file` (103), `claude::parse` (101); `pipelines` (96) close. | `usage/merge.rs:62-164`, `usage/claude.rs:128-228`, `usage/tools.rs:137-232` | 4a (`parse` in 3a) |
| S9 | Tool classification is nested string matching; belongs in a table. | `usage/tools.rs:453-520` | 4a |
| S10 | `serde_json::Value` where a struct fits. | `statusline.rs:21`, `hook/mod.rs:35`, `context.rs:127-133`, `cmd_doctor.rs:187-191` | 4a |
| S11 | `"unknown".to_owned()` default repeated. | `cmd_context.rs:48`, `cmd_session.rs:40-42`, `cmd_usage.rs:207-209` | 4a |
| S12 | `limits/mod.rs` mixes the status API with forecasting and budget wiring; `usage/merge.rs` mixes file folding with agent stats. | `limits/mod.rs`, `usage/merge.rs` | 4a |
| S13 | `tests/cli.rs` is 1166 lines. | `tests/cli.rs` | 4a |
| S14 | Two non-recursive directory listers. | `ledger/scan.rs:17`, `usage/walk.rs:9` | 3a |
| S15 | Hook state lock resembles acus-guard `hints.rs`; see AUDIT-acus G8. | `hook/state.rs` | 6 |

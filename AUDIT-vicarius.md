# Audit: vicarius

Read-only audit from 2026-11-11. Line counts are raw file lines including tests. Work order and
owners live in [AUDIT.md](AUDIT.md); the step ids below point there.

## State

No file over a limit (`config.rs` 558 with tests after about line 327). The only non-test
`expect` (`config.rs:301`) carries an `#[expect]`. Uses `log`, `claude_settings`, `hooks`,
`which`, `update`, `hint` and `render` as intended.

## Findings

| # | Finding | Evidence | Step |
|---|---------|----------|------|
| V1 | Atomic write implemented twice; `config.rs` writes to a fixed `json.tmp`, so two concurrent writers collide. | `config.rs:281-293`, `agents/sync.rs:121` | 3c |
| V2 | Signal handling duplicated with pons. | `cmd_serve.rs:66-83` | 3c |
| V3 | I/O errors discarded with `let _ =`. | `proxy/record.rs:195,197` | 3c |
| V4 | Unverified: an `await` held under a `std::sync::RwLock`, and task cancellation in the refresher. | `gate.rs:169`, `refresh.rs:247-264` | 3c |
| V5 | Hand-rolled plain-TCP HTTP client while the proxy already uses hyper and reqwest. | `status_client.rs` (227) | 4c |
| V6 | Request body rewrite edits whole `Value` trees. | `proxy/inline_tools.rs` (323), cap at `proxy/handler.rs:27` | 4c |
| V7 | Unverified: whether the `ps` call in doctor runs on an async path. | `doctor/mod.rs:149` | 4c |
| V8 | Port rollback and restart sequences are long hand-written step lists. | `cmd_config/mod.rs:343-414`, `cmd_service/mod.rs:216-337` | later |

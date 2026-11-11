# Audit: pons

Read-only audit from 2026-11-11. Line counts are raw file lines including tests. Work order and
owners live in [AUDIT.md](AUDIT.md); the step ids below point there.

## State

No file over a limit (`log/mod.rs` 500 with tests from line 195), no function over 100 lines
(largest `doctor/view.rs::legacy_json` 67). Uses `cli`, `render`, `exit`, `hint`, `log::Writer`,
`census_snapshot`, `which`, `pkg` and `update` as intended.

Security: `clientCapabilities` handling (`engine/initialize.rs:13-36`, `engine/wire.rs:10-36`,
`engine/route.rs`) is security-sensitive. No default may loosen; every change there needs tests
and a review before commit.

## Findings

| # | Finding | Evidence | Step |
|---|---------|----------|------|
| P1 | Signal handling duplicated with vicarius. | `proxy/mod.rs:268-308`, vicarius `cmd_serve.rs:66-83` | 3b |
| P2 | `unsafe` (process-group kill) is not in its own module. | `proxy/mod.rs:315-360` | 3b |
| P3 | `proxy/mod.rs` (408) holds signals, process group and the run loop together. | `proxy/mod.rs`, `proxy/teardown.rs` | 3b |
| P4 | About 40 `serde_json::Value` pointer lookups across the engine; a typed inbound view would replace them while forwarding stays byte for byte. | `engine/updates.rs:15-34,89`, `engine/route.rs:132,230-233`, `engine/response.rs:135,165`, `engine/wire.rs:48-74` | 4b |
| P5 | `initialize` re-serializes the whole message. | `engine/initialize.rs:36` | 4b |
| P6 | Fold state machine works on untyped values; many clones. | `fold/entry.rs` (401), `fold/background.rs` (11 clones, `poll` 60 lines) | 4b |
| P7 | Hand-built `legacy_json` in doctor. Remove it: Luca is the only pons user, so nothing depends on its bytes. | `doctor/view.rs:300-367` | 5b |
| P8 | Local JSONC stripping; core has no equivalent and no second user. | `doctor/zed.rs:21` | stays |

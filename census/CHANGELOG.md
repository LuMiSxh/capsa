# Changelog

## [Unreleased]

### Added

- `census usage`, moved from `acus usage`.
- `census usage` cost in $ per model, session and role, from a list-price table (`src/usage/prices.rs`).
  Requests of models without a price count as unpriced.
- `census usage` cache hit rate per session and role, and expired-cache rewrites and unexpected
  busts per role (`--agents`).
- `census usage --handbacks`: spawns per ISO week and a hand-back heuristic (a senior-worker spawn
  within 30 minutes after a worker spawn of the same session).
- `census usage --compare YYYY-MM-DD`: the report before and after a UTC date, with the change.
- `census usage`: compactions per session with the context size before each one.

# SPDX-License-Identifier: MPL-2.0

set shell := ["bash", "-euo", "pipefail", "-c"]

# List the available commands.
default:
    @just --list

# Run the acus CLI with the given arguments, e.g. `just acus run 'cargo test'`.
[group('Development')]
acus *args='':
    cargo run -q -p acus-cli -- {{ args }}

# Run the pons ACP proxy with the given arguments.
[group('Development')]
pons *args='':
    cargo run -q -p pons -- {{ args }}

# Run the census CLI with the given arguments.
[group('Development')]
census *args='':
    cargo run -q -p census -- {{ args }}

# Run the vicarius proxy with the given arguments.
[group('Development')]
vicarius *args='':
    cargo run -q -p vicarius -- {{ args }}

# Check the local acus install, config, skill and hooks.
[group('Development')]
doctor:
    cargo run -q -p acus-cli -- self doctor

# Run the workspace tests. Nextest is used; if not installed, fall back to `cargo test`.
[group('Checks')]
test *args='':
    #!/usr/bin/env sh
    set -eu
    if command -v cargo-nextest >/dev/null 2>&1; then
        cargo nextest run --workspace {{ args }}
    else
        cargo test --workspace {{ args }}
    fi

# Lint the whole workspace with warnings denied.
[group('Checks')]
lint:
    cargo clippy --workspace --all-targets --all-features --locked --keep-going -- -D warnings

# Format the workspace with rustfmt.
[group('Checks')]
format:
    cargo fmt --all

# Check the workspace format without writing.
[group('Checks')]
format-check:
    cargo fmt --all --check

# Fail when a tracked file still holds a git conflict section.
[group('Checks')]
conflicts:
    python3 scripts/check-conflicts.py

# Run the full check, as in CI: format check, lint, conflict markers and test.
[group('Checks')]
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked --keep-going -- -D warnings
    python3 scripts/check-conflicts.py
    just test --all-features --locked

# Run the acus benchmarks.
[group('Checks')]
bench *args='':
    cargo bench -p acus-cli {{ args }}

# Run the skill trigger and task evals; pass e.g. `--dry-run`.
[group('Checks')]
evals *args='':
    python3 scripts/skill-evals.py {{ args }}

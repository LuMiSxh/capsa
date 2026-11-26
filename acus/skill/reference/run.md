# run

```bash
acus run 'CMD' [--tail N] [--grep RE] [--explain] [--timeout SECS]
acus run
```

Runs a shell command, merges stdout and stderr, drops build noise, and prints the code each
`path:line` reference points at.

- Detects cargo test, cargo nextest, Swift XCTest and swift-testing, pytest, go test, vitest and
  jest, Bun, Deno, RSpec, PHPUnit, CTest and dotnet test. Their passing, skipped and progress
  lines go, and a green summary becomes `N passed, M skipped` in the footer; failures, their
  output and the failing summary stay.
- For an unknown tool, a run of four or more similar status lines folds to its first and last
  line plus a count, and repeated lines collapse to `line (×N)`. A run that carries an error,
  warning or failure is never folded. Nested XCTest `Executed` summaries fold to the last one.
- Without CMD it runs `[run] command` from the config, else the project's test command:
  `cargo nextest run` when nextest is installed, else `cargo test -q`; then Package.swift,
  go.mod, package.json or pytest.
- `--tail N` keeps the last N lines plus every error and failure block; `--grep RE` keeps only
  matching lines plus the code they point at. `-A`, `-B` and `-C N` add context to `--grep`
  matches, as grep does.
- `--max-output N` (default 200) keeps every failure block and the last N lines, and counts the
  rest in a gap line. `--drop RE` removes matching lines, `--raw` keeps noise, and `--cut N`
  clips lines.
- `--explain` names the detected tool and how many lines each kind dropped.
- `--max-refs N` (default 8), `--max-lines N` (default 40) and `--context N` (default 3) shape
  the code shown around a reference.
- `--timeout SECS` kills a hung command and keeps what it printed, exiting 124.
- `--log FILE` reads a saved log, such as a background task's output, through the same filters.
- `--head N` stops after N printed lines.

The exit code is the command's own; a failing command still prints its filtered output.

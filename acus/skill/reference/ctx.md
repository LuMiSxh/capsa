# ctx and run

## ctx

```bash
acus ctx 'CMD' [--tail N] [--grep RE] [--timeout SECS]
acus ctx
```

Runs a shell command, merges stdout and stderr, drops build noise, and prints the code each
`path:line` reference points at.

- Without CMD it runs `[ctx] command` from the config, else the project's test command
  (Cargo.toml, Package.swift, go.mod, package.json, pytest).
- `--tail N` keeps the last N lines plus every error and failure block; `--grep RE` keeps only
  matching lines plus the code they point at. `-A`, `-B` and `-C N` add context to `--grep`
  matches, as grep does.
- `--drop RE` removes matching lines, `--raw` keeps noise, `--max-output N` caps kept lines
  (default 200), and `--cut N` clips lines.
- `--max-refs N` (default 8), `--max-lines N` (default 40) and `--context N` (default 3) shape
  the code shown around a reference.
- `--timeout SECS` kills a hung command and keeps what it printed, exiting 124.
- `--log FILE` reads a saved log, such as a background task's output, through the same filters.
- `--head N` stops after N printed lines.

The exit code is the command's own; a failing command still prints its filtered output.

## run

```bash
acus run <<'EOF'
find 'fn parse' --block
show src/a.rs#f src/b.rs:10-30
outline src/lib.rs
EOF
```

- One command per line, without the `acus` prefix, or a JSON array of argument lists.
- It exits 0 when at least one line succeeded and lists failures in a `failed: N of M lines`
  footer; it exits 2 only when every line failed or `--fail-fast` stopped at the first.
- Output is cut fairly within 24k characters; each cut names the follow-up with `--head N` (or
  `acus run --full`). `--full` lifts the budget, and `--head` and `--cut` cap every line.

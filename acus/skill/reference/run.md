# run

```bash
acus run 'CMD' [--tail N] [--grep RE] [--timeout SECS]
acus run
```

Runs a shell command, merges stdout and stderr, drops build noise, and prints the code each
`path:line` reference points at.

- Without CMD it runs `[run] command` from the config, else the project's test command
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

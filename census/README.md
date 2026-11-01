# census

Usage analytics for Claude Code and Codex: tokens, cost, tool calls and subagent roles from the
session transcripts. Limits, context awareness and the hook are planned in [PLAN.md](PLAN.md).

## Install

```sh
cargo install --locked --path census
```

`./install.sh census` installs the release build instead.

## Commands

`census usage` reads `~/.claude/projects` and `~/.codex/sessions` (or `CLAUDE_CONFIG_DIR` and
`CODEX_HOME`) and prints a summary.

| Option | Meaning |
|---|---|
| `--days N` | Only transcripts modified in the last N days |
| `--project TEXT` | Only sessions whose working directory contains TEXT |
| `--top N` | Rows per table (default 10) |
| `--claude-dir DIR` | Claude Code projects directory |
| `--codex-dir DIR` | Codex sessions directory |
| `--tools` | How agents search, read, edit and build, per category |
| `--guard` | `acus guard` decisions per rule and the escaped calls, from acus's `guard.jsonl` |
| `--agents` | One row per subagent role: spawns, models, requests and cost |

Output is agent text by default; `--human` adds colour on a terminal, `--json` prints the report as
JSON. `--format agent|human|json` is the same choice.

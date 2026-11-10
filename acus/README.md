<div align="center">
<img src="../.github/assets/acus.svg" width="112" height="112" alt="Acus logo" />

# Acus

**Code search, outline and editing for coding agents.** Hits are grouped by the function or type they sit in, `--block` prints those bodies, symbols are addressed by name instead of guessed line numbers, and every truncation names the command that shows the rest.

[![Rust 1.99+](https://img.shields.io/badge/Rust-1.99%2B-41c4ad?style=flat-square&labelColor=23252a)](https://www.rust-lang.org/)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-41c4ad?style=flat-square&labelColor=23252a)
[![MPL-2.0 license](https://img.shields.io/badge/license-MPL--2.0-41c4ad?style=flat-square&labelColor=23252a)](../LICENSE)
[![Version](https://img.shields.io/badge/version-0.19.0-41c4ad?style=flat-square&labelColor=23252a)](CHANGELOG.md)

**[capsa](../README.md)** · [Changelog](CHANGELOG.md)

</div>

Acus is one of the [tools in capsa](../README.md#tools). It answers a coding agent's search, read and edit chain in one call.

## Install

Acus is distributed from [LuMiSxh/capsa](https://github.com/LuMiSxh/capsa), which replaces the former LuMiSxh/Acus repository. See [Install](../README.md#install) in the capsa README for the installer options.

macOS and Linux, acus only:

```sh
curl -fsSL https://raw.githubusercontent.com/LuMiSxh/capsa/main/install.sh | sh -s -- acus
```

Windows (PowerShell) installs every tool; to get only acus, save the script and run `.\install.ps1 acus`:

```powershell
irm https://raw.githubusercontent.com/LuMiSxh/capsa/main/install.ps1 | iex
```

Both installers put the newest `acus-v*` release in `~/.local/bin` (`CAPSA_INSTALL_DIR` changes that), add it to `PATH` and install the agent skill to `~/.claude/skills/acus/` and `~/.agents/skills/acus/`. `CAPSA_NO_SKILL=1` (or `ACUS_NO_SKILL=1`) skips the skill. Then run `acus self setup` to register the hooks; see [Guard and agent setup](#guard-and-agent-setup).

Build from source, with the toolchain pinned in `rust-toolchain.toml` (see [Build from source](../README.md#build-from-source)):

```sh
cargo install --path acus/crates/acus-cli
acus self setup
```

## Commands

Everything is top-level except the management commands under `acus self` (see [Self](#self)).

| Command                       | What it does                                                                                                                                                                                                                                                                                 |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `acus find PATTERN [PATH...]` | Regex search, hits grouped by enclosing symbol; `--block`/`-b` adds the bodies. `-w -l -t -u -i -F -g` work as in rg; `-c -o -v -m --max-count -A/-B/-C --include --exclude` as in grep; `-x` whole lines, `-L` follows symlinks, `-U` multiline. An address (`path:A-B`, `path:N-`, `path:-N`, `path:LINE`, `path#Symbol`) in place of a path searches only that range, with file-absolute line numbers (`-g`/`-t` are ignored). `--cut N` (`--max-columns N`) clips hit lines (default 200); `--last N` keeps the last N hits per file |
| `acus files [PATH...]`        | Lists files, respecting `.gitignore`: `-g GLOB`, `--hidden`, `-u` (also ignored files), `-t TYPE`, `-L`, `--lines` (line count per file and a total; `--json`), `--depth N` (`--max-depth`) limits the walk, `--head N` and `--cut N` cap the listing |
| `acus outline PATH...`        | Symbols of a file or directory with kinds and line ranges; `-k fn,struct` keeps only those kinds                                                                                                                                                                                             |
| `acus show ADDR...`           | Symbols, line ranges, whole files, `path:-N` (last N lines) or `path:N-` (line N to the end), numbered; `REV:path` reads a file as of a git revision (`REV:./path` from the current directory). A whole file over 400 lines shows its outline and first 60 lines; `--full` prints every line, an explicit range or symbol is never capped, and a call stops near 24k characters naming the address that shows the rest |
| `acus blame ADDR`             | Who last changed each run of lines, one line per commit; `--json`. Reads the working tree, so `REV:path` is not accepted                                                                                                                                                                    |
| `acus follow ADDR`            | Recursive callers (`-d N`, default 2); `--down` lists callees resolved to repo definitions; `--all` lifts the caps; `--head N` and `--cut N` cap the output; `--json`. Syntactic only: calls are matched by name, so dynamic and trait dispatch, function references and implicit calls are missed. A `?` edge is a name match, shown but not followed |
| `acus patch`                  | Applies a patch from stdin, all or nothing, and prints the written lines; `--check` validates, `--diff` prints a unified diff (see [Patches](#patches))                                                                                                                                      |
| `acus ctx ['COMMAND']`        | Runs a build or test; drops progress noise, colour codes, passing tests and library frames, and shows the code behind each `path:line`. Without a command it runs the project's tests (detected from Cargo.toml, Package.swift, go.mod, package.json or pytest files). `--tail N` keeps the last N lines and every failure block; `--grep RE` keeps matching lines (`-A/-B/-C` add context); `--timeout SECS` kills the command, keeps the output so far and exits 124; `--head N` and `--cut N` cap the output. When output is cut, the full log is saved to a temp file and its path printed |
| `acus diff [REV] [PATH...]`   | Uncommitted changes per function or type, with untracked files; `-p` adds the lines, collapses symbols moved elsewhere in the same diff, outlines new files and caps each file at 80 lines (`--full` prints every body line and lifts the cap); `--staged` and `A..B` work as in git; without REV the first line names the branch, upstream position, staged, unstaged and untracked counts, conflicts and any merge, rebase or cherry-pick, and a footer names the staged changes; `--stat` prints `path +A -R` per file with a total, `--name-only` one `STATUS path` line each, and `--conflicts` the unmerged paths with their conflict line ranges (exit 1 when there are none) |
| `acus log [REV] [PATH...]`    | One line per commit, `hash subject (age)`; `-n` sets the count (default 15); `--head N` and `--cut N` cap the output; `REV` can be `A..B`                                                                                                                                                    |
| `acus run`                    | Several of the above in one process: one command per stdin line, or a JSON list of argument lists                                                                                                                                                                                           |
| `acus map [DIR...]`           | Directory tree with file and line counts within a token budget (`--budget 400`): the largest directories open first, chains roll up, small directories list their top-level symbols; tests and docs open last. Several paths map as one tree; `--head N` and `--cut N` cap the output |
| `acus hook`                   | Claude Code hook: `SessionStart` and `SubagentStart` print the skill, and `PreToolUse` (Bash, PowerShell, Read, Grep and Glob) rewrites calls it can convert exactly into acus calls, refuses the rest and names the acus command to use. See [Guard and agent setup](#guard-and-agent-setup) and [docs/guard.md](docs/guard.md) |

`acus log --oneline` is accepted as a no-op.

Addresses look like `path#Type::method`, `path:10-40`, `path:10`, `path:-20`, `path:150-` or just `path`. A unique suffix such as `#method` is enough, `Type.method` works too, and config keys nest with `::` (`config.yaml#server::port`). Symbols are understood for Rust, Swift, TypeScript (with TSX), JavaScript, Svelte, Python, Shell, C, CSS and SCSS (not on Windows), HTML, XML and plists, Markdown headings, and TOML, JSON and YAML keys; other files work with `find`, `files` and line ranges.

Output is a compact agent format by default. `--agent`, `--human` and `--json` choose it explicitly, before or after the command; `--human` adds colour on a terminal and `NO_COLOR` turns colour off. Exit codes: 0 for results or success, 1 for nothing found, 2 for errors, which carry a `hint:` line. `find`, `files` and `log` with no matches exit 1, `diff` with no changes exits 0 (`--conflicts` exits 1 when it finds none), and their empty results serialize as `null` in JSON. `ctx` passes on the exit code of the command it ran.

## Patches

`acus patch` edits text the way Claude's Edit tool does. A `SEARCH`/`REPLACE` block under `*** Update File:` replaces one exact substring, part of a line or several lines; `*** Replace Symbol:` swaps out a whole function or type by name:

```
*** Begin Patch
*** Update File: src/lib.rs
<<<<<<< SEARCH
    let x = 1;
=======
    let x = 2;
>>>>>>> REPLACE
*** Replace Symbol: src/lib.rs#Parser::reset
+    fn reset(&mut self) {
+        self.pos = 0;
+    }
*** End Patch
```

- The old text must occur exactly once. With no match the error shows the nearest lines; with several it lists their line numbers, so add a neighbouring line. If the text is not found as written, acus also accepts it with `acus show` line-number prefixes stripped, ignoring trailing whitespace or leading indentation, and prints a `note:`. `--fuzzy` (or `[patch] fuzzy = true`) adds one final pass that ignores tabs vs spaces, runs of whitespace and blank lines; REPLACE is written as given and a `note:` gives each match's range.
- Blocks and files apply all or nothing, and each file is replaced atomically. When several blocks fail, one run reports every failing block with its nearest match, and nothing is written. Before writing, each file that tree-sitter supports is parsed: a patch that adds syntax errors fails and names the first new error's line (`--allow-broken` writes anyway).
- `+` prefixes only the new lines of Replace Symbol, Insert Before/After, Add File and Append File. A multi-line SEARCH matches a CRLF file as LF and writes CRLF back, with a `note:`.
- `--check` validates and reports what would change; `--diff` prints a unified diff (with `--check`, a preview). `--fmt` (or `[patch] fmt = true`) runs the file's formatter if one is installed (rustfmt, ruff or black, prettier, swift-format, gofmt) and prints the formatted lines.
- Further directives (`acus patch --help` has the syntax):
    - `*** Replace All: PATH...` changes every occurrence, also part of a line, in files, directories and globs (`*.py`); it fails if a block matches nowhere. A `<<<<<<< REGEX` block is a Rust regex per file with `(?m)`, and `${1}` is the replacement's group.
    - `*** Insert Before:` and `*** Insert After:` take `path#Symbol` or `path:N` and put the `+` lines above the symbol's doc comments and attributes, or below the symbol.
    - `*** Append File: PATH` adds the `+` lines after the file's last line, with one newline between them; a missing file needs `*** Add File:`.
    - `*** Copy File: SRC` with `*** To: DST` copies a file and fails if DST exists.
    - `*** Delete Symbol:` and `*** Move Symbol:` carry doc comments and attributes along. A move goes `*** Before:` or `*** After:` another symbol, or `*** To:` the end of a file (created if needed).
    - `*** Delete Lines: PATH:A-B` and `*** Move Lines: PATH:A-B` cut or move lines by number. The next line is `*** Expect:` with the exact text of line A, so a stale range fails; a move then takes `*** After: PATH:N` (0 for the file start) or `*** After: PATH#Sym`. Line numbers refer to the file before the patch, so these run first.
    - `*** Resolve Conflict: PATH --ours` resolves every `<<<<<<<` / `=======` / `>>>>>>>` section of a file with one side (`--theirs`, `--both` for ours then theirs, or `--base` for the diff3 `|||||||` part). `PATH:LINE --theirs` resolves only the section that contains line LINE, so one patch can pick different sides. All line directives and resolutions of one file count from the file before the patch, so they do not shift each other; a delete range or a move destination inside a section being resolved fails.

## Guard and agent setup

Agents fall back to grep and cat when a skill is only listed by its description. Loading the skill at session start and letting a hook steer the rest fixes most of that.

**Claude Code.** `acus self setup` installs the skill and registers the three hooks in `settings.json` (`$CLAUDE_CONFIG_DIR`, else `~/.claude`), keeping a `.bak` of the old file; a rerun changes nothing when they are current. `--target claude|codex|all` picks the skill directories (`codex` is refused, since the hooks are Claude Code's), and `--dry-run` prints what would change.

- `SessionStart` (`acus hook`) prints the skill at every start, `/clear` and compaction. It has no project map, because [overviews in context files did not help agents find the relevant files and raised cost](https://arxiv.org/abs/2602.11988); agents run `acus map` for the layout.
- `SubagentStart` (`acus hook`, no matcher, so every subagent) adds the skill to a subagent's context. `acus self setup` installs it; an entry already scoped to agent types, e.g. `Explore|Plan`, is kept as it is.
- `PreToolUse` runs the guard (`acus hook`) on Bash, PowerShell, Read, Grep and Glob. It rewrites a command into an exact acus call when it can, and otherwise refuses it and names the acus command. Literal `sed -i` edits of one file become `acus patch`, `acus … | head -N` becomes `--head N`, and `cat`, `tail` and `grep` of a background task's output file become `acus ctx --log FILE`. The rules are in [docs/guard.md](docs/guard.md).

Escape hatches: a refused command runs anyway with a `command ` prefix (Bash) or a trailing `# acus-skip` comment (both shells). Every decision, escapes included, is logged to `guard.jsonl` next to the config file; `census usage --guard` counts them per rule. The hook command ends in `|| true`, so calls pass when acus is missing or when `hook` is listed under `[commands] disabled`.

Hooks apply to subagents too, but subagents do not see SessionStart output. Preload the skill in each custom agent's frontmatter, and give read-only agents `Bash` so they can run it. The built-in Explore agent cannot preload skills; a custom agent with the same role can.

```yaml
tools: Read, Glob, Grep, Bash
skills:
    - acus
```

**Codex.** Codex has no session hooks, and its hooks cannot rewrite commands, so they always refuse. Add this line to `~/.codex/AGENTS.md`:

```markdown
Search and read code with `acus` instead of grep/rg/find/cat/sed -n (`acus find 'PAT' --block`, `acus outline PATH`, `acus show path#Symbol path:A-B`), edit with one `acus patch`, and run builds and tests via `acus ctx` (alone for the project's tests, or `acus ctx 'COMMAND'`).
```

## Configuration

`config.toml` is read from `$ACUS_CONFIG`, else from `~/Library/Application Support/acus/` (macOS), `%APPDATA%\acus\` (Windows) or `$XDG_CONFIG_HOME/acus/`.

```toml
[commands]
disabled = ["usage"]

[ctx]
drop = ["^warning: unused"]                         # extra output lines acus ctx hides
command = "just test"                               # what `acus ctx` alone runs; default: detected

[patch]
fmt = true                                          # as if every patch had --fmt

[guard]
hint_every = 10                                     # repeated guard hints: 1st, 11th, 21st… call per session and rule; 1 = every time
```

`acus ctx --drop REGEX` does the same for one run. `ACUS_DISABLE` (comma-separated, e.g. `ACUS_DISABLE=usage`) adds to `[commands] disabled`.

Languages are compiled in through the features `lang-rust`, `lang-swift`, `lang-ts`, `lang-python`, `lang-markdown`, `lang-svelte`, `lang-data`, `lang-shell`, `lang-c`, `lang-css`, `lang-scss`, `lang-html` and `lang-xml`; the guard's shell grammars through `guard-bash`, `guard-zsh` and `guard-powershell`. All are on by default. Without a grammar the guard falls back to a simpler splitter that guards less precisely and rewrites nothing for Bash. The zsh grammar adds about 4.8 MB and the PowerShell one about 1 MB. To keep only what you need, pass `--no-default-features` and list the features, for example `cargo install --path crates/acus-cli --no-default-features --features lang-rust,lang-python,lang-shell,guard-bash,cmd-update`.

## Self

`acus self <action>` manages acus itself.

| Command               | What it does                                                                                                                                                                                 |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `acus self setup`     | Installs the skill and registers `acus hook` for SessionStart and PreToolUse in Claude Code's settings; `--target claude\|codex\|all`, `--dry-run`                                       |
| `acus self doctor`    | Checks the install, config, skill, hooks and guard files, one line per check with a hint for each failure; `--json`. Exit 1 when a check fails, 0 when only warnings remain                    |
| `acus self skill`     | Prints the bundled agent skill (`--full` prints the whole skill); `--install --target claude\|codex\|all` writes it                                                                          |
| `acus self update`    | Replaces the binary with the latest GitHub release when newer, then reinstalls the skill; `--check` exits 0 when an update exists, 1 when current; `--force` reinstalls the latest release. Sends a token when one is available (`GITHUB_TOKEN`, else `gh auth token`). It only lifts GitHub's rate limit; none is required |
| `acus self uninstall` | Removes the installed skill and the binary; `--dry-run` only lists them                                                                                                                      |

Hooks registered in `~/.claude/settings.json` stay after an uninstall: remove the acus entries by hand (they end in `|| true`, so they are harmless meanwhile).

## Benchmarks

Measured with [hyperfine](https://github.com/sharkdp/hyperfine) on an Apple M4. The small corpus is tokio (564 files), the large one a full Cargo registry (80,197 files, 1.8 GB). Run locally with `cargo bench -p acus-cli` (Criterion benches in `crates/acus-cli/benches`).

| Task                                   | acus   | Compared with            |
| -------------------------------------- | ------ | ------------------------ |
| Literal search without hits, tokio     | 9.6 ms | rg 10.1 ms, grep 20.0 ms |
| Literal search without hits, registry  | 2.08 s | rg 2.21 s, grep 6.66 s   |
| Regex search with hits, registry       | 1.60 s | rg 1.64 s                |
| Find a definition plus its body, tokio | 9.8 ms | ast-grep 54.5 ms         |
| Outline of every file, tokio           | 81 ms  | ctags 67 ms              |
| Outline of every Rust file, registry   | 15.7 s | ctags 20.0 s             |

What the agent reads matters more than milliseconds: for one tokio function, `acus show` returns 384 bytes, `acus outline` 974, `acus find --block` 5,132, `cat` 26,550 and `rg -C20` 58,501.

## Development

For the workspace layout, recipes and conventions, see [AGENTS.md](AGENTS.md). CI runs these checks on Ubuntu, macOS and Windows:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Release notes go in [CHANGELOG.md](CHANGELOG.md); the release process is under [Releases](../README.md#releases) in the capsa README.

## License

Acus is licensed under the [Mozilla Public License 2.0](../LICENSE). Copyright © 2026 LuMiSxh. Claude Code and Codex belong to their respective owners.

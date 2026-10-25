<div align="center">

# Acus

**Code search, outline and editing for coding agents.** Acus answers a search, read and edit chain in one call. Output is grouped by symbol, and every truncation names the command that shows the rest.

*rem acu tetigisti*: you hit it with the needle.

[![Rust 1.99+](https://img.shields.io/badge/Rust-1.99%2B-477acc?style=flat-square&labelColor=23252a)](https://www.rust-lang.org/)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-477acc?style=flat-square&labelColor=23252a)
[![MPL-2.0 license](https://img.shields.io/badge/license-MPL--2.0-477acc?style=flat-square&labelColor=23252a)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.17.0-477acc?style=flat-square&labelColor=23252a)](CHANGELOG.md)

**[Install](#install)** · [Commands](#commands) · [Agent setup](#agent-setup) · [Changelog](CHANGELOG.md)

</div>

## Why

Coding agents spend a surprising share of their turns finding code: a search, then a read of the file, then a second read because the first window was too small. Each step costs a round trip and puts lines into the context that nobody needed.

Acus answers that chain in one call. Search hits come grouped by the function or type they sit in, `--block` prints those bodies, and symbols are addressed by name instead of guessed line numbers:

```console
$ acus find 'fn spawn_blocking' --block
$ acus show src/runtime/handle.rs#Handle::spawn_blocking src/lib.rs:40-80
```

When output is cut short, the last line names the command that shows the rest.

## Install

Acus is distributed from the private repository [LuMiSxh/capsa](https://github.com/LuMiSxh/capsa), which replaces the former repository LuMiSxh/Acus. Its root `install.sh` and `install.ps1` need a GitHub token with read access: set `GITHUB_TOKEN`, for example `export GITHUB_TOKEN=$(gh auth token)` after `gh auth login`.

macOS and Linux, acus only:

```sh
curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" \
  https://raw.githubusercontent.com/LuMiSxh/capsa/main/install.sh | sh -s -- acus
```

Windows (PowerShell), which installs every tool; to get only acus, save the script and run `.\install.ps1 acus`:

```powershell
$h = @{ Authorization = "Bearer $env:GITHUB_TOKEN" }
irm -Headers $h https://raw.githubusercontent.com/LuMiSxh/capsa/main/install.ps1 | iex
```

Both installers download the newest `acus-v*` release to `~/.local/bin` (`CAPSA_INSTALL_DIR` changes that), add it to `PATH` and run `acus self skill --install`, which writes the agent skill to `~/.claude/skills/acus/` and `~/.agents/skills/acus/`. Set `CAPSA_NO_SKILL=1` (or `ACUS_NO_SKILL=1`) to skip the skill. Release binaries include `acus decide`.

From source, with Rust 1.88 or newer:

```sh
cargo install --path crates/acus-cli --features cmd-decide
acus self skill --install
```

Commands about acus itself are `acus self <action>`; everything else is top-level.

`acus self update` replaces the binary where it is installed with the latest release when that is newer (checked against the release's SHA-256), then reinstalls the skill so it matches the binary; `--check` only reports, `--force` reinstalls the latest release. It needs the same token as the installers. The new binary is written beside the old one, synced and renamed over it; a symbolic link is resolved and the real binary is replaced. After a source build, run `acus self skill --install` yourself.

`acus self uninstall` removes the installed skill and then the binary; `--dry-run` prints what would go. Hooks registered in `~/.claude/settings.json` stay: remove the acus entries by hand (they end in `|| true`, so they are harmless meanwhile).

## Agent setup

An installed skill is only listed by its description, so agents often fall back to grep and cat. Loading the skill at session start and letting a hook steer the remaining habits fixes most of that.

**Claude Code.** One command registers both hooks in `~/.claude/settings.json`. It keeps a `.bak` of the old file and everything else in it, and a rerun replaces the acus entries instead of duplicating them:

```sh
acus self skill --install --hooks
```

- `SessionStart` prints the skill at every start, `/clear` and compaction. It has no project map, because [overviews in context files did not help agents find the relevant files and raised cost](https://arxiv.org/abs/2602.11988). Agents run `acus map` when they need the layout.
- `PreToolUse` runs `acus guard` on Bash, PowerShell, Read, Grep and Glob calls. It rewrites a command into an exact acus call when it can, and otherwise refuses it and names the acus command to use. Literal `sed -i` edits of one file become `acus patch`, `acus … | head -N` becomes `--head N`, and `cat`, `tail` and `grep` of a background task's output file become `acus ctx --log FILE`. The rules are in [docs/guard.md](docs/guard.md).

Escape hatches:

- A refused command runs anyway with a `command ` prefix (Bash) or a trailing `# acus-skip` comment (both shells).
- Every decision, escapes included, is logged to `guard.jsonl` next to the config file. `acus usage --guard` counts them per rule and lists the escaped calls.
- The hook command ends in `|| true`, so calls pass when acus is missing or `guard` is disabled in the configuration.

Hooks apply to subagents too. Subagents do not see SessionStart output, so preload the skill in each custom agent's frontmatter, and give read-only agents `Bash` so they can run it:

```yaml
tools: Read, Glob, Grep, Bash
skills:
  - acus
```

The built-in Explore agent cannot preload skills; a custom agent with the same role can.

**Codex.** Codex has no session hooks, and its hooks cannot rewrite commands, so they always refuse. Add a line to `~/.codex/AGENTS.md`:

```markdown
Search and read code with `acus` instead of grep/rg/find/cat/sed -n (`acus find 'PAT' --block`, `acus outline PATH`, `acus show path#Symbol path:A-B`), edit with one `acus patch`, and run builds and tests via `acus ctx` (alone for the project's tests, or `acus ctx 'COMMAND'`).
```

## Commands

| Command | What it does |
| --- | --- |
| `acus find PATTERN [PATH...]` | Regex search, hits grouped by enclosing symbol; `--block` adds the bodies; `-w`, `-l`, `-t`, `-u`, `-i`, `-F` and `-g` work as in rg; `-c`, `-o`, `-v`, `-m`, `-A/-B/-C`, `--include` and `--exclude` work as in grep; `-x` matches whole lines, `-L` follows symlinks and `-U` allows multiline patterns; an address (`path:A-B`, `path:N-`, `path:-N`, `path:LINE` or `path#Symbol`) in place of a path searches only that range, with file-absolute line numbers, and ignores `-g`/`-t`; `--cut N` clips each hit line to N characters (default 200); `--last N` keeps the last N hits per file, after `-m` |
| `acus files [PATH...]` | Lists files, respecting `.gitignore`: `-g GLOB`, `--hidden`, `-u` (also ignored files), `-t TYPE`, `-L` (follow symlinks), `--lines` (line count per file and a total; `--json` for scripts) |
| `acus outline PATH...` | Symbols of a file or directory with kinds and line ranges; `-k fn,struct` keeps only the given kinds |
| `acus show ADDR...` | Symbols, line ranges, whole files, the last N lines (`path:-N`) or the lines from N to the end (`path:N-`), numbered; `REV:path` reads a file as of a git revision (`REV:./path` from the current directory) |
| `acus blame ADDR` | Who last changed each run of lines of a symbol or range, one line per commit; `--json` for scripts. Reads the working tree, so `REV:path` is not accepted |
| `acus follow ADDR` | Who calls a symbol, recursively (`-d N`, default 2); `--down` lists what it calls, resolved to definitions in the repo. Syntactic only: calls are matched by name, so dynamic and trait dispatch, function references and implicit calls are missed. A `?` edge is a name match, shown but not followed; `--all` lifts the caps; `--json` for scripts |
| `acus patch` | Applies a patch from stdin, all or nothing, and prints the written lines; `--check` validates and reports what would change, `--diff` prints a unified diff |
| `acus ctx ['COMMAND']` | Runs a build or test, drops progress noise, colour codes, passing tests and library stack frames, and shows the code behind every `path:line` in its output. Without a command it runs the project's tests (detected from Cargo.toml, Package.swift, go.mod, package.json or pytest files). `--tail N` keeps the last N lines plus every failure block, `--grep RE` keeps the matching lines, and `-A/-B/-C` add context to `--grep`. `--timeout SECS` kills the command, keeps the output so far and exits 124. When output is cut, the full log is saved to a temp file and its path printed |
| `acus diff [REV] [PATH...]` | Uncommitted changes per function or type, with untracked files; `-p` adds the lines, `--staged` and `A..B` work as in git; without REV, a footer names the staged changes |
| `acus log [REV] [PATH...]` | One line per commit with date, author and changed lines; `-n` sets the count (default 15); `REV` can be `A..B` |
| `acus run` | Several of the above in one process, one command line per stdin line (or a JSON list of argument lists) |
| `acus usage` | Token, cost and tool-call statistics from Claude Code and Codex transcripts; `--tools` shows how agents search, read, edit and build; `--guard` counts `acus guard` decisions per rule |
| `acus decide QUESTION` | Yes/no, choice or score answer from a Jev-compatible API; without an API key it warns and exits 2 |
| `acus map [DIR]` | Directory tree with file and line counts within a token budget (`--budget 400`): the largest directories open first, chains and single-file directories are rolled up, and small directories list their files' top-level symbols; tests and docs open last |
| `acus self skill` | Prints the bundled agent skill for a SessionStart hook; `--install` writes it for Claude Code and Codex, `--install --hooks` also registers the hooks |
| `acus self update` | Replaces the binary with the latest GitHub release when newer and reinstalls the skill; `--check` exits 0 when an update exists, 1 when current |
| `acus self uninstall` | Removes the installed skill and the binary; `--dry-run` only lists them |
| `acus guard` | PreToolUse hook for Bash, PowerShell, Read, Grep and Glob. Rewrites the calls it can convert exactly into acus calls and refuses the rest, naming the acus command to use. See [docs/guard.md](docs/guard.md) |

`acus log --oneline` and `acus diff --stat` are accepted as no-ops, for git habits.

Addresses look like `path#Type::method`, `path:10-40`, `path:10`, `path:-20` (the last 20 lines), `path:150-` (line 150 to the end) or just `path`. A unique suffix such as `#method` is enough, `Type.method` works too, and config keys nest with `::` (`config.yaml#server::port`). A symbol is shown with its doc comments, attributes and decorators.

Exit codes are 0 for results or success, 1 for an empty result when that command treats emptiness as no match, and 2 for errors. `find`, `files` and `log` with no matches exit 1, while `diff` with no changes exits 0. Empty `find`, `files`, `log` and `diff` results serialize as `null`. Errors carry a `hint:` line. `ctx` passes on the exit code of the command it ran.

Output defaults to a compact agent format. `--json` and `--human` are there for scripts and people.

Symbols are understood for Rust, Swift, TypeScript (with TSX), JavaScript, Svelte, Python, Shell (`.sh`, `.bash`, `.zsh`), C (`.c`, `.h`), CSS and SCSS (not on Windows), HTML, XML and plists (`.xml`, `.plist`, `.xib`, `.storyboard`; `Info.plist#CFBundleIdentifier` resolves a plist key), Markdown headings, and TOML, JSON and YAML keys. Every other file still works with `find`, `files` and line ranges.

## Patches

`acus patch` edits text the way Claude's Edit tool does. Under `*** Update File:`, a SEARCH/REPLACE block replaces one exact substring, part of a line or several lines. `*** Replace Symbol:` swaps out a whole function or type by name:

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
*** Add File: notes.md
+hello
*** Delete File: old.rs
*** End Patch
```

The old text must occur exactly once in the file. With no match the error shows the nearest lines; with several it lists their line numbers, so add a neighbouring line to the block to narrow the search. Several blocks per file and several files per patch apply all or nothing. If the text is not found as written, acus also accepts it with `acus show` line-number prefixes stripped, ignoring trailing whitespace or ignoring leading indentation (tabs vs spaces, with REPLACE written in the file's indentation), and prints a `note:`. `+` prefixes only the new lines of Replace Symbol, Insert Before/After and Add File; SEARCH/REPLACE text has none.

Before writing, acus parses each file that tree-sitter supports. A patch that gives a file more syntax errors than it had fails as a whole and names the first new error's line. `--allow-broken` writes the file anyway. A multi-line SEARCH matches a CRLF file as LF text, writes the REPLACE with CRLF line endings and prints a `note:`.

Agents that would otherwise write a `sed -i` line or a small Python script get a few more directives:

```
*** Replace All: src *.py
<<<<<<< SEARCH
old_name(
=======
new_name(
>>>>>>> REPLACE
<<<<<<< REGEX
fn (\w+)_old\(
=======
fn ${1}_new(
>>>>>>> REPLACE
*** Delete Symbol: src/lib.rs#unused
*** Move Symbol: src/lib.rs#helper
*** After: src/util.rs#main
*** Insert Before: src/lib.rs#parse
+// Parses one expression.
*** Copy File: src/lib.rs
*** To: src/lib_copy.rs
```

SEARCH/REPLACE is the format Aider made popular. `*** Replace All: PATH [PATH...]` changes every occurrence, also part of a line, in the space-separated files, directories and globs after it (a glob such as `*.py` matches below the current directory), and fails if a block matches nowhere. A block is `<<<<<<< SEARCH` (literal, whitespace-exact, may span lines) or `<<<<<<< REGEX` (a Rust regex run per file with `(?m)`; `${1}` in the replacement). `acus patch --help` shows the syntax. `*** Insert Before:` and `*** Insert After:` take `path#Symbol` (or `path:N`) and put the `+` lines above the symbol's doc comments and attributes, or below the symbol. `*** Copy File: SRC` with `*** To: DST` copies a file and fails if DST exists. `Delete Symbol` and `Move Symbol` carry doc comments and attributes along; a move goes `*** Before:` or `*** After:` another symbol, or `*** To:` the end of a file, which is created if needed. `*** Delete Lines: PATH:A-B` and `*** Move Lines: PATH:A-B` cut or move lines by number; the next line is `*** Expect:` with the exact text of line A, so a stale range fails instead of cutting other lines, and a move then takes `*** After: PATH:N` (0 for the file start) or `*** After: PATH#Sym`. Line numbers refer to the file before the patch, so these run first and the other directives apply in order to the result.

Nothing is written unless every block applies. Files are replaced atomically. `--check` only validates and reports what would change, and `--diff` prints a unified diff instead of the written lines (with `--check` it is a preview). `--fmt` (or `[patch] fmt = true`) runs each written file through its formatter if one is installed (rustfmt with the crate's edition, ruff or black, the project's prettier, swift-format, gofmt) and prints the formatted lines, so line numbers and context stay true for the next patch.

## Numbers

Measured with [hyperfine](https://github.com/sharkdp/hyperfine) on an Apple M4. The small corpus is tokio (564 files), the large one a full Cargo registry (80,197 files, 1.8 GB).

| Task | acus | Compared with |
| --- | --- | --- |
| Literal search without hits, tokio | 9.6 ms | rg 10.1 ms, grep 20.0 ms |
| Literal search without hits, registry | 2.08 s | rg 2.21 s, grep 6.66 s |
| Regex search with hits, registry | 1.60 s | rg 1.64 s |
| Find a definition plus its body, tokio | 9.8 ms | ast-grep 54.5 ms |
| Outline of every file, tokio | 81 ms | ctags 67 ms |
| Outline of every Rust file, registry | 15.7 s | ctags 20.0 s |

What the agent has to read matters more than milliseconds. For one function in tokio:

| Approach | Bytes |
| --- | --- |
| `acus show handle.rs#Handle::spawn_blocking` | 384 |
| `acus outline handle.rs` | 974 |
| `acus find --block` (all hits with bodies) | 5,132 |
| `cat handle.rs` | 26,550 |
| `rg -C20` (hits with guessed context) | 58,501 |

Criterion benchmarks for the core paths live in `crates/acus-cli/benches`.

## Configuration

`config.toml` is read from `$ACUS_CONFIG`, otherwise from `~/Library/Application Support/acus/` on macOS, `%APPDATA%\acus\` on Windows and `$XDG_CONFIG_HOME/acus/` elsewhere.

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

[decide]
enabled = true
url = "https://openrouter.ai/api/alpha/decisions"   # or https://api.typesafe.ai/v1/systemone
model = "~typesafe/jev-latest"                      # "jev-latest" when talking to TypeSafe directly
api_key_env = "OPENROUTER_API_KEY"                  # TYPESAFE_API_KEY when talking to TypeSafe directly
```

`acus ctx --drop REGEX` does the same for a single run. The other settings can also come from the environment: `ACUS_DISABLE=usage,decide`, `ACUS_DECIDE_ENABLED`, `ACUS_DECIDE_URL`, `ACUS_DECIDE_MODEL` and `ACUS_DECIDE_API_KEY_ENV`.

Language support is compiled in through the features `lang-rust`, `lang-swift`, `lang-ts`, `lang-python`, `lang-markdown`, `lang-svelte`, `lang-data`, `lang-shell`, `lang-c`, `lang-css`, `lang-scss`, `lang-html` and `lang-xml`, all on by default. Leave some out for a smaller binary.

The guard reads shell commands with tree-sitter grammars, chosen by the features `guard-bash`, `guard-zsh` and `guard-powershell`, also all on by default. Under a zsh login shell (`$SHELL`, which is where Claude Code runs Bash-tool commands on a Mac) the guard uses the zsh grammar, otherwise the Bash one; the PowerShell tool uses the PowerShell grammar. A build without a grammar falls back to a simpler hand-written splitter for that shell, which guards less precisely and rewrites nothing for Bash. The zsh grammar adds about 4.8 MB to the binary and the PowerShell one about 1 MB; for example `cargo install --path crates/acus-cli --no-default-features --features lang-rust,lang-python,lang-shell,guard-bash` keeps only the Bash grammar (list the `lang-*` and `cmd-update` features you want too, because `--no-default-features` drops them all).

## Development

For the workspace layout, recipes and conventions, see [AGENTS.md](AGENTS.md). CI runs these checks on Ubuntu, macOS and Windows:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo bench -p acus-cli
```

The benchmark runs locally only.

Releases are built by the `Release` workflow: bump the version in `Cargo.toml`, add a `CHANGELOG.md` section, merge, then run the workflow with the tag (`v0.14.0`). It builds macOS (arm64, x86_64), Windows (x86_64) and Linux (x86_64, arm64) archives with checksums.

| Crate | Role |
| --- | --- |
| `acus-cli` | The `acus` binary, output formats and the `run` batch mode |
| `acus-walk` | Parallel file walking that respects `.gitignore`, plus globs |
| `acus-search` | Search with enclosing-symbol context, built on the ripgrep crates |
| `acus-syntax` | tree-sitter outlines and symbol lookup |
| `acus-edit` | Patch parsing and atomic application |
| `acus-usage` | Transcript statistics |
| `acus-decide` | Client for Jev-compatible decision APIs |

## License

Acus is licensed under the [Mozilla Public License 2.0](LICENSE).

Copyright © 2026 LuMiSxh. Claude Code, Codex and Jev belong to their respective owners.

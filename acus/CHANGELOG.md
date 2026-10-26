# Changelog

## [0.18.1]

### Fixed

- On Windows the guard expanded `~` to a home path with backslashes, so `cat ~/x/a.rs` was not rewritten; the expanded path now uses `/`.
- `guard.jsonl` lines written by parallel hooks could interleave into lines that are not JSON: a line and its newline were written separately. Each decision is now one write call, and a test appends from four threads at once and checks that every line parses.
- `acus guard` read a `command ` prefix on a pipe stage as an unknown command, so `grep X f | command head -30` ran unchanged. The stage is now read as `head`, and the search is rewritten to `acus find X f | command head -30`.

## [0.18.0]

### Added

- `acus usage --tools` shows the size of tool results per category, including each `acus` subcommand: characters and share of the group (`result-chars`, `result-share-of-group`; `result_chars` and `result_share` in `--json`). A result is attributed to the call that it answers.
- Shell writes (`cat > file <<EOF`, `cat >> file`, `tee file`, `printf … > file`) are counted as the `edit-shell` category, not as reads.
- `*** Delete Lines: PATH:A-B` deletes lines A to B (1-based, as `acus show` prints them) and `*** Move Lines: PATH:A-B` moves them, followed by `*** After: PATH:N` (N 0 for the file start), `*** After: PATH#Sym` or the same in another file. Each line directive needs `*** Expect:` with the exact text of line A on the next line, so a stale range fails with the actual line. Line numbers refer to the file before the patch; overlapping ranges, a target inside the moved lines and ranges past the end are refused.

### Changed

- `acus guard` rewrites an `acus patch` whose written lines are thrown away into `acus patch -q`: a `| head` or `| tail` as the last stage, or a redirect to `/dev/null` (also with `2>&1` or `&>`). `-q` prints the summary and errors, not the written lines. Other pipes and redirects are left as they are.
- The agent skill (`acus self skill`) makes `acus patch -q` the default when the written lines are not needed, names `*** Delete Lines` and `*** Move Lines` with their `*** Expect:` line, and says `-g '!dist'` excludes a path. It no longer asks agents to write a patch to a file before `cd DIR &&`; the heredoc is the normal form.
- `acus patch --help` lists `*** Delete Lines`, `*** Move Lines` with `*** Expect:`, and `*** After:`.
- `acus patch` echoes a `*** Replace Symbol:` body longer than 40 lines as its first and last 5 lines, with a `… N more lines` marker that names the `acus show` range; `--json` still lists every line. With `--fmt` the formatted lines are echoed in full.

### Fixed

- Codex `acus` calls were counted as 0 when their command was stored as escaped JSON or in a JS tool body; the command is now decoded first.
- The guard kept the pipe of a heredoc command inside an `&&` or `||` list on the wrong segment, so `cd X && acus patch <<'P' | tail` was not rewritten to `-q`; the pipe now stays on the heredoc command and later list elements no longer inherit its stdin.
- The patch syntax check no longer refuses C files with GNU `__attribute__((…))` or `__asm__(…)` groups, which tree-sitter-c reports as errors, nor JSON files that already use comments or trailing commas (JSONC such as Zed's `settings.json`); a strict JSON file that gains a trailing comma is still refused.

## [0.17.0]

### Added

- `--agent` selects agent output, as `--format agent` does; `CAPSA_HUMAN=1` now selects human output, as in the other capsa tools.
- `acus self uninstall [--dry-run]` removes the installed skill (`SKILL.md` under `~/.claude/skills/acus/` and `~/.agents/skills/acus/`, and the directory when it is then empty) and the binary. It refuses a binary not named `acus` and checks that before removing anything. Hooks in `~/.claude/settings.json` are left.
- `acus self update` writes the new binary beside the old one, syncs it and renames it over; a symbolic link is resolved to the real binary, and a link or non-file found at replacement time is refused with a hint. The checksum is verified before anything is written.

### Changed

- Commands about acus itself are `acus self <action>`: `acus update` is `acus self update` and `acus skill` is `acus self skill` (same flags, including `--install` and `--hooks`). There are no aliases. Every other command stays top-level. The hooks that `self skill --install --hooks` registers run `acus self skill`; registered `acus skill` entries are replaced on the next install. Update the SessionStart hook in `~/.claude/settings.json` when you install this version.
- The release-update code lives in `capsa-core` (feature `update`), shared with pons and velum; `cmd-update` now forwards to it and still gates `self update` and `self uninstall`.
- The guard keeps its tree-sitter parser per thread. Measured with the guard benchmark: no gain beyond noise (within 1%), kept for simplicity.
- Command-line errors from clap are rendered through `capsa-core` like every other error (a JSON object under `--json`), including inside `acus run`, where an unknown flag of `find` now carries the same hint as at the top level.
- Internal: `cmd_patch.rs` is `cmd_patch/` (`syntax.rs`, `tests.rs`) the rules of `acus-syntax` are `rules/mod.rs` and `rules/markup.rs`; `acus-guard` has directory modules for `read`, `search`, `rewrite`, `rules` and `build`.
- The crates are marked `publish = false`.
- Errors in agent output print the cause chain on one line (`error: top: cause`), with `hint:` on its own line after it; human output keeps the `caused by:` layout.
- `--json` errors are printed on stderr as a JSON object with `error`, `causes` and `hint` fields.
- `self update` reads releases from the private LuMiSxh/capsa repository, takes the newest `acus-v*` tag and needs `GITHUB_TOKEN` or `gh auth login`.

### Removed

- `install.sh` and `install.ps1` moved to the root of LuMiSxh/capsa; the copies in `acus/` are gone.

## [0.16.1]

### Added

- `--cut N` on `acus ctx` clips long output lines, as on `show`, `outline`, `diff` and `find`; headers and footers stay whole.

### Changed

- `acus find` leaves out the symbol label when the hit is the symbol's first line (its definition); a label stays when the symbol has hits in its body.

### Fixed

- A refused build or `acus ctx` pipe into `grep`, `cut` and `tail` offers a `Send instead` command only when it covers every stage: a `cut` before `tail` moves behind the ctx flags, and a stage no flag does (`cut` before `grep`, `tr`) or a later non-filter (`awk`) names no command.

## [0.16.0]

### Added

- `--head N` on `show`, `outline`, `diff` and `find` stops after N output lines and says how many were cut; `--cut N` on `show`, `outline` and `diff` clips long lines.
- `acus ctx --log FILE` reads a saved log, such as a background task's output file, through the same filters (`--tail`, `--grep`, `--drop`, context), with a note when the file is still being written.

### Changed

- ctx strips the job, step and timestamp prefixes of `gh run view --log` lines and drops `##[endgroup]`.
- `acus map` reads the symbols of several files in parallel (28 ms → 14 ms on this repository).
- Guard: `acus show|outline|find|diff … | head -N` becomes the same call with `--head N`; `--help`, `--json`, `--format`, `--head` and `--cut` calls are left alone.
- Guard: `cat`, `tail -N` and `grep` of a background task's `tasks/*.output` file become `acus ctx --log FILE` with the matching flags.
- Guard: `sed -i ['' ] 's/OLD/NEW/g' FILE` with literal text on one regular file becomes an `acus patch` Replace All, left to the agent's permission prompt. Unlike sed, the patch fails and writes nothing when OLD is not found. Directories, symlinks, regex characters, other flags and several files keep the refusal.
- Guard: `git log --oneline` runs without a hint.
- Guard: `gh run view --log` / `--log-failed` piped through filters becomes `acus ctx`.
- Guard: `timeout N BUILD | …` becomes `acus ctx 'BUILD' --timeout SECS …`; a timed build whose filters do not fold runs as written.
- Guard: a quoted `<` or `>` in a filter no longer blocks the ctx rewrite.
- Guard: `grep --exclude-dir={a,b}` (also `--include`/`--exclude`) with plain names becomes one `-g` per name.
- `acus patch` echoes at most 120 written lines and names the remaining regions as `acus show` addresses.
- `acus follow` analyses candidate files in parallel (123 ms → 56 ms for callers, 151 ms → 52 ms with `--down` on this repository).
- `acus find` parses only the files it prints and reuses read buffers and searchers (27 ms → 18 ms on this repository, 154 ms → 49 ms on a larger one).
- Guard: `acus ctx '…' --tail T | head -H` and `BUILD | tail -T | head -H` fold to the command without the head when H >= T; H < T keeps the refusal.
- Internal: the guard is its own crate, `acus-guard`; the Cargo features `guard-bash`, `guard-zsh` and `guard-powershell` are unchanged. Tree-sitter grammar versions are set once in the workspace. `acus follow`'s call-graph engine lives in `acus-search`.
- Guard: a grep or rg whose pattern or path the shell expands (`$VAR`, `${…}`, `$(…)`, backticks) runs as written with a hint to `acus find`, where it used to be refused.
- Guard: a build or `acus ctx` piped into a chain with `sort` or `uniq` runs as written, and so do `acus ctx '…'` and a build piped into stacked greps that no single ctx flag set expresses (`| grep a | grep b`); chains that fold, such as `| grep -v x | grep y`, still fold, and other unfoldable filters (`awk`, `sed`, `cut`) are still refused.

### Fixed

- `acus patch` writes through a symlink: an edit to a file that is a symlink, such as `CLAUDE.md` → `AGENTS.md`, changes the target and keeps the link, where it used to replace the link with a regular file.
- Guard: a grep or rg option word that is wholly quoted, such as `'-i'` or `"--exclude=target"`, is read as the option it is once the shell removes the quotes, where it used to become a file operand; a quoted word with braces, globs or `$` is still refused.
- Guard: a refused pipe into several filters names every one that no ctx flag does (`sort`, `uniq`), in order, where it used to name only the first.
- Symbol directives refuse a name that a type and its impl share, such as `enum Mode` with `impl Mode`, and write nothing; they used to act on only the impl and leave the type behind.

## [0.15.4]

### Changed

- Guard: filters piped after `acus ctx '…'` fold into its flags instead of refusing the command: `grep RE` → `--grep`, `grep -v RE` → `--drop`, a final `head -N` → `--max-output N`, a final `tail -N` → `--tail N`; a trailing `cut -c…` stays behind the rewritten command and a `2>&1` after it is dropped, since ctx already merges stderr. `sort`/`uniq`, filters with no exact ctx form, and a pipe inside the quoted command still refuse; the `Send instead:` line is given only when it covers every filter of the chain. A `2>&1` inside the quoted build command no longer blocks the build-pipe rewrite.
- Guard: grep over several files or directories, with `--include=GLOB` (→ `-g`), flags after the pattern, or glob paths, and `cat` of several files or a glob, are rewritten to one `acus find` / `acus show` call; globs are left for the shell to expand.
- Guard: guard.jsonl keeps calls up to 8000 characters (was 300), so logged calls can be replayed whole.

### Fixed

- Diff: untracked files are marked `?` in `acus diff` and in the `--json` `status`, like git's `??`, instead of `A`. `A` stays for files that are staged or new in a `REV..REV` diff.
- Guard: `grep -A/-B/-C … | grep -v …` after a build or `acus ctx` is refused instead of folded, because ctx drops lines before it cuts the context window. A grep without `-r` on a directory, and an `--include`/`--exclude` glob with braces, are no longer rewritten to a recursive or brace-expanding `acus find`.
- `acus show` exits 1 when one of its addresses fails (missing path or symbol), like cat, after printing the others.

## [0.15.3]

### Changed

- Guard: more read pipelines fold into one `acus show` range instead of being refused: `sed -n 'A,Bp' F | head -M` / `| tail -M`, `head -N F | head -M` / `| tail -M`, `sed -n 'N,$p' F | head -M`, and `cat F | sed -n 'A,Bp'` (also `'N,$p'`, `'Np'`). A tail over a range is clamped to the file's last line, so `head -20 F | tail -5` on a 10-line file shows lines 6-10.
- Guard: `grep … F | head -N` is rewritten to `acus find … --max-hits N`, and `| tail -N` on one file to `--last N`. With context flags (`-A`/`-B`/`-C`) the refusal names the find with `--max-hits N`.
- Guard: grep basic-regex escapes convert to `acus find`: `\b`, `\B`, `\w`, `\W`, `\s`, `\S` pass through, `\<` and `\>` become `\b`, and `$` or `<`/`>` inside a quoted pattern no longer block the rewrite. `grep -n 'R1\b\|R12' F` becomes `acus find 'R1\b|R12' 'F'`.
- Guard: a read piped into grep becomes one `acus find` over that range: `sed -n 'A,Bp' F | grep X` → `acus find 'X' 'F:A-B'`, likewise `cat F`, `head -N F`, `tail -N F` and `sed -n 'N,$p' F`, with a trailing `| head -N` as `--max-hits N`.
- Guard: a build piped through a filter chain becomes one `acus ctx` call: `grep RE` → `--grep`, `grep -v RE` → `--drop` (repeatable), a final `head -N` → `--max-output N` and `tail -N` → `--tail N`. A chain that contains `sort` is refused instead: ctx only drops adjacent repeats, not repeats sorted apart.
- Guard: a display filter (`head`, `tail`, `cut -c`) after a folded read or search stays behind the rewritten acus command instead of blocking the rewrite.
- Guard: refusals end with a `Send instead:` line holding the full replacement command when one exists, and a `Blocked:` line naming the refused segment otherwise.

### Fixed

- Guard: help and version output (`--help`, `-h`, `help`, `--version`, `-V`) of cargo, swift, `acus ctx` and other build tools is no longer treated as a build, so `acus ctx --help | grep X` passes.
- Guard: `tail -n +N F | head -M` is clamped to the file's last line like the other range folds.

## [0.15.2]

### Added

- `path:N-` is an address (line N to the end of the file, past the end as an out-of-range `A-B`) in `acus show`, `find` and `blame`. The guard rewrites `tail -n +N FILE` (also `tail +N`, `--lines=+N` and `cat FILE | tail -n +N`) and `sed -n 'N,$p' FILE` to `acus show 'FILE:N-'`, and `tail -n +N FILE | head -M` (also `cat FILE | tail -n +N | head -M`) to the line range N to N+M-1 of FILE.

### Fixed

- `acus follow` finds the callers and callees of functions that share a line, as in minified JS: each call is matched to its enclosing symbol by byte offset, not by line.
- Guard: `cat FILE 2>/dev/null | head -N` (also `2> /dev/null`, `2>&1`, and `| tail -N`) is rewritten to `acus show 'FILE:1-N' 2>/dev/null` (`:-N` for tail), as `head -N FILE 2>/dev/null` already is. A refusal of a `cat | head` pair that cannot be rewritten names its range when it has one, and never the whole file. `tail -n +N` reads from line N, not N lines, so it is rewritten to the open-ended `FILE:N-` range instead of a count.
- Guard: the per-session hint counter takes a short lock, so parallel hook calls no longer lose counts. A call that cannot get the lock within 50 ms shows its hint and leaves the count alone.

## [0.15.1]

### Changed

- Guard: a repeated pure hint (git log, git diff, edit scripts, reads and searches in loops) is shown on the first call of its rule per Claude Code session and then on every `[guard] hint_every`th call (default 10). The calls in between pass silently. A SessionStart after a compaction or `/clear` shows the hints again. Rewrites and refusals still show on every call.

### Fixed

- Guard: `grep -c`, `rg -c` and `--count` run as written. They were rewritten to `acus find … -c`, whose per-file output and total line differ from grep's count, so no rewrite, refusal or hint is given for a count.
- `acus files -g GLOB` (and `acus find -g`) matches a glob without a `/` at any depth, as `rg -g` does: `-g FooTests.swift` finds `Tests/A/B/FooTests.swift`. A glob with a `/` stays relative to the search root. An empty result with `-g` names what was searched.
- `acus diff` with no revision names the staged changes in a footer (`staged: N files +A -D (acus diff --staged)`) and prints it instead of `(no changes)` when only staged changes exist. The exit code stays 0; JSON output is unchanged.
- `acus patch` counts `+N -M` per file from the file's text before and after the patch, so context lines repeated unchanged in REPLACE no longer count as changes. The counts match `git diff --numstat`, also for `--check` and `--fmt`; a file touched by several directives, including an edit and a move of the same file, gets one line with its net count.

## [0.15.0]

### Added

- `acus follow ADDR` lists the callers of a symbol, recursively (`-d N`, default 2); `--down` lists what it calls, resolved to definitions in the repo. Calls are matched by name in the source, so only sure calls are followed; `?` edges are shown but not followed. `--json` for scripts.
- `acus show REV:path` reads a file as of a git revision (commit, branch or tag) with the same addresses: `REV:src/a.rs` counts from the repo root as git does, `REV:./src/a.rs` from the current directory.
- `acus blame path#Sym`, `path:A-B` or `path:-N` says who last changed each run of lines, one line per commit; `--json` for scripts. It reads the working tree, so `REV:path` is not accepted.
- `acus outline --kind` (`-k fn,struct`, comma-separated or repeated) keeps only symbols of those kinds; a match's parents stay as context lines.
- `acus patch --diff` prints a unified diff instead of the written lines; with `--check` it is a preview that writes nothing.
- `acus patch` SEARCH falls back to matching ignoring leading indentation (tabs vs spaces), writes REPLACE in the file's indentation and prints a `note:`.
- Guard: zsh and PowerShell are read by their own tree-sitter grammars. Claude Code runs Bash-tool commands in the user's login shell, so under zsh (`$SHELL`) the guard now parses `${(f)x}`, `=(cmd)`, glob qualifiers such as `*(.)`, `repeat 3 { … }` and `|&`, and guards the commands in them instead of letting the call through unparsed. The PowerShell tool is parsed by tree-sitter-powershell, with the old splitter as the fallback for a line the grammar rejects (for example `Get-Content a.rs,b.rs`). Cargo features `guard-bash`, `guard-zsh` and `guard-powershell` (all default) pick the grammars at build time; without them the hand-written splitter is used. The release binary grows by about 5.8 MB.
- Guard corpus: each case has a `dialect` (`bash`, `zsh` or `powershell`); the golden test runs it on the backend that dialect selects and skips cases whose grammar is not built. A hidden `acus guard --replay FILE` runs a file of cases through the guard.
- `acus find` takes an address in place of a path: `path:A-B`, `path:-N`, `path:LINE` or `path#Symbol` searches only that range, with line numbers counted from the start of the file. Addresses mix with plain paths, and `-g`/`-t` are ignored for them. Replaces `sed -n A,Bp f | grep x`.
- `acus find --cut N` clips each printed hit line to N characters (default 200), and `--last N` keeps only the last N hits of each file, applied after `-m`. Together they replace `grep x log | cut -c1-200 | tail -30`.
- `acus files --lines` appends each file's line count and a total; `--json` gives the same as data. Replaces `wc -l`.

### Changed

- `acus patch --check` reports what would change (`would update a.rs +1 -1`), and `--fmt` with `--check` is an error.
- Guard: Bash commands are split with tree-sitter-bash instead of a hand-written scanner. Each segment now knows where its stdin and stdout go and how deeply it is nested, so a rewrite replaces exact byte ranges. A command with `$( )` or backticks is no longer left unrewritten as a whole: `x=$(cat a.md); cat a.md` rewrites the outer `cat`. `FOO=1 cat a.md` becomes `FOO=1 acus show 'a.md'`. A command the grammar cannot parse (an unterminated quote, zsh-only syntax under the Bash grammar) runs unchanged.
- Guard: the golden corpus test no longer depends on `$HOME`; `~` expands through the scope, which a test fixes.
- Guard: `git diff` whose output reaches the agent is rewritten to `acus diff`: `git diff [REV] [-- PATHS]` becomes `acus diff -p …`, and `git diff --stat …` becomes `acus diff …`. `--staged`/`--cached` convert too. `A...B`, several revisions, other flags, a diff that gates `&&`/`||`, and a piped diff keep the hint.
- Guard: `BUILD | tail -N; echo "exit=${PIPESTATUS[0]}"` is rewritten to `acus ctx 'BUILD' --tail N; echo "exit=$?"`, so the pipeline converts too; zsh `${pipestatus[1]}` works the same. The echo must directly follow with `;`; other shapes keep today's behaviour.
- Guard: a read or search inside a loop or group whose output reaches the agent gets a hint naming the acus command (`for f in a.rs; do head -3 a.rs; done` hints `acus show 'a.rs:1-3'`), instead of passing silently. Captured or piped output in a group still runs unchanged.
- Guard: `cat f | head -N` becomes `acus show f:1-N`, and `sed -n '3p;7,9p' f` becomes `acus show f:3 f:7-9`. A `sed -n` script with a regex address is refused. `head -c` and `tail -c` pass.
- Guard: `acus ctx … | tail -N` becomes `--tail` with the smaller count: `--tail 5 | tail -3` gives `--tail 3`, and `--tail 3 | tail -5` gives `--tail 3`.

### Fixed

- Guard: a segment whose stdout is captured as data is no longer rewritten, refused or hinted. This covers `$( )`, backticks, `<( )`, `>( )` and every stdout redirect (`>`, `>>`, `1>`, `&>`, `>&2`, `>|`). Before, `{ cat old.md; } > new.md` became `acus show`, which put line numbers into the file.
- Guard: git hints and the other hints and refusals inside a loop or group whose output reaches the agent are hints, never refusals or rewrites, so `for f in a b; do git diff -- $f; done` gets the `acus diff` hint and runs.
- `acus blame` runs git from the file's directory, so an absolute path works from any directory. Lines not committed yet show `00000000`, author `-` and summary `uncommitted` (also in `--json`), instead of git's `Not Committed Yet` text.
- Guard: display-filter pipelines (`| head`, `| grep`, `| sort`, …) after a search, read or `acus ctx` count as output the agent reads, so they are refused or hinted again. Real consumers still pass: `xargs`, `sh`, `python`, `jq`, `awk`, `sed`, `while read`, `tee` and file redirects.
- Guard: a build command that contains `$` or a backtick is refused, not rewritten, because `acus ctx` would expand the text a second time. The suggestion still shows the double-quoted form; plain builds keep single quotes.
- Guard: rewrites and suggestions keep `$VAR` and globs unquoted, so the shell expands them as it did in the original command (`grep foo docs/*.md | head -3` suggests `acus find 'foo' docs/*.md`).

## [0.14.0]

### Added

- Guard converter: `acus guard` rewrites a command into an exact acus call and refuses only what it cannot convert. Claude Code runs the rewritten call in the same turn. `grep` and `rg` become `acus find`, `cat`, `head` and `sed -n` become `acus show`, and `BUILD | filter` becomes `acus ctx` flags. `grep -r` becomes plain `acus find`, which reports the hidden and ignored files it skipped; `-A/-B/-C N`, `-NUM`, `--include`, `--exclude` and `--exclude-dir` become `acus find` flags; `fgrep` becomes `-F`. POSIX basic and extended patterns are translated to Rust regex, or the call is not converted when the meaning could differ. `VAR=x`, quotes and `time` in a build are kept, and a build that uses `$` is left alone.
- Guard: `tail -N FILE` and `tail -n N FILE` become `acus show FILE:-N`; `head`, `head -N FILE` and `cat -n FILE` become `acus show`.
- Guard: `find DIR -name GLOB` (and `-iname`) and `rg --files` become `acus files`, and `fd` gets a hint that names it.
- Guard: `grep -x`, `grep -R`, `rg -L` (and `--follow`), and `-U` / `--multiline` map to the matching `acus find` flags (`-x`, `-L`, `-U`).
- `acus files [PATH…] [-g GLOB] [--hidden] [-u] [-t TYPE] [-L]` lists files under the given paths, with the ignore rules of `find`.
- `acus show path:-N` prints the last N lines of a file.
- `acus find` takes `-x` (whole-line matches), `-L` (follows symlinks, as in rg) and `-U` (multiline patterns), and `--files` gets a hint to `acus files`.
- `acus ctx --timeout SECS` kills the command's process group after SECS seconds, keeps the output so far and exits 124.
- New languages with outlines and symbols: Shell (`.sh`, `.bash`, `.zsh`), C (`.c`, `.h`), CSS (`.css`), SCSS (`.scss`, not on Windows), HTML (`.html`, `.htm`) and XML (`.xml`, `.plist`, `.xib`, `.storyboard`). `Info.plist#CFBundleIdentifier` resolves a plist key.
- `acus patch` checks each written file that tree-sitter supports before writing anything: a patch that gives a file more syntax errors than it had fails as a whole, naming the first new error's line with a few lines around it. `--check` reports it too. Files already broken and not made worse pass, and unsupported languages are skipped. `--allow-broken` writes the file anyway and prints a `note:`.
- `acus find` takes grep's `-A N`, `-B N` and `-C N`: N lines of context around each hit, overlapping ranges merged and non-adjacent groups split by `--`. Context lines print as `N-` and hits as `N`. An explicit `-A` or `-B` overrides `-C` on its side, and the last repeat wins. `--block` conflicts with them; `-o`, `-c`, `-l` and `--json` do not print context.
- `acus ctx CMD --grep RE` takes `-A N`, `-B N` and `-C N`, which keep the output lines after, before or around each match, like grep: groups that do not touch are separated by `--`, the last of repeated flags wins, and `-A`/`-B` override `-C`. Without `--grep` they are an error.

### Changed

- `acus log --oneline` and `acus diff --stat` are accepted as hidden no-ops, so git habits do not fail.
- `acus find` searches outside the result lock, and `acus show` parses each file once.
- Internal: `guard`, `ctx` and `apply` are directory modules; patch directives come from one table (`DIRECTIVES`), languages from one table (`Lang::ALL`); the tests are split by area.
- `acus patch` matches a multi-line SEARCH in a CRLF file against its LF text, writes the REPLACE with CRLF line endings, and prints `note: matched with CRLF line endings`.

### Fixed

- The guard no longer refuses or rewrites a search or read whose output goes into a pipe (`grep -l X | xargs …`, `grep X | wc -l`, `cat f | python3 …`), is redirected to a file, or is `grep -q`: it runs as written. A build piped into `grep -q`, or into a filter and then another command, runs as written too.
- The guard no longer mistakes a pattern in double quotes for its text when it holds `\`, `$` or a quote: such calls are not converted, and a `'` inside `"…"` is no longer dropped.
- The guard's rewrite of `grep` and `rg` into `acus find` keeps `-c`, `-l`, `-o`, `-v`, `-w`, `-i`, `-F`, `-m`, `-t`, `-g` and `--hidden`, and refuses a search with a flag it cannot reproduce exactly (such as `-P` or `-z`) instead of dropping it: a `grep -c` had been rewritten into the matching lines. `sed -n` ranges that are reversed or start at 0 are refused too.

## [0.13.0]

### Added

- `acus find PAT -` searches standard input.
- `acus skill --full` prints the whole skill.

### Changed

- `acus skill`, which the SessionStart hook runs, prints a short core (routing table and patch skeleton, under 200 words) instead of the full skill.
- The guard rewrites refused segments of compound commands in place when each has an exact acus equivalent, including commands with heredocs, segments ending in `2>/dev/null` or `2>&1`, and `acus ctx … | head -N`/`tail -N` (now `--max-output`/`--tail`). A segment without an exact equivalent still refuses the whole command.
- `acus find` and `acus show` with several paths warn about missing ones and go on with the rest; only when none exists is it an error.
- `acus patch` skips a SEARCH/REPLACE block whose SEARCH equals REPLACE, after checking it matches once, and prints a `note:`. A patch of only such blocks is still an error.
- `acus find` accepts repeated `-A`/`-B`/`-C`; the last wins.
- The skill says that `+` prefixes only Replace Symbol, Insert Before/After and Add File lines.

## [0.12.0]

### Added

- `acus skill --install` takes `--agent claude|codex|all` (default `claude`, which writes only `~/.claude/skills/acus/`); `acus update` reinstalls the skill for the agents that already have it.

### Changed

- `acus patch` edits text with SEARCH/REPLACE blocks under `*** Update File:`, with the semantics of Claude's Edit tool: the old text is a substring (part of a line or several lines) that must occur exactly once in the file. With no match the error shows the nearest lines; with several it lists their line numbers. Blocks of one file and files of one patch apply all or nothing. If the text is not found as written, acus also accepts it with `acus show` line-number prefixes stripped, or ignoring trailing whitespace, and prints a `note:`.
- The skill, the README and `acus patch --help` teach SEARCH/REPLACE as the one way to edit text; `*** Replace All:` is unchanged.

### Removed

- `@@` hunks with `-`/`+`/context lines in `*** Update File:`, and the `@@ anchor` line that could narrow a block. The 0.11.0 hunk fallbacks (split hunks, partial-line matches, tolerant whitespace matching) are removed too. A patch using hunk lines fails with an error that shows the SEARCH/REPLACE form. The `*** End of File` marker is gone with them.

## [0.11.0]

### Added

- `acus patch` falls back to two more matches after exact and whitespace-tolerant matching fail, and prints a `note:` for each: a hunk whose lines are not adjacent in the file is split between its `-`/`+` groups and applied as separate parts when every part matches once, in order; and a hunk of k `-` lines followed by k `+` lines whose `-` texts each occur once inside a line replaces that text in place.
- Paths starting with `~` are expanded to the home directory in every command's path arguments, in `acus run` lines and in patch file paths.

### Changed

- `acus guard` rewrites a Bash command instead of refusing it when every offending segment has an exact acus replacement: reads (`cat`, `head -n`, `sed -n`, `grep`, `rg`) run as `acus show`/`acus find` with `permissionDecision: allow`; build pipes (`CMD | tail/head/grep`) run as `acus ctx` and go through the normal permission flow. The agent is told what ran through `additionalContext`. Codex hook calls keep the refusals. `acus usage --guard` counts rewrites separately.
- The skill leads with an "Instead of … run" table and adds batching advice and hunk rules.

## [0.10.0]

### Added

- `acus usage --agents`: one row per subagent role (from `agentType` in the subagent's meta file, else the parent's Agent call; top-level sessions are `main`) with spawns, models, median requests and first-request context per spawn, weighted cost, share and median cost per spawn. The default JSON report gains an `agents` key.

## [0.9.0]

### Added

- `acus patch` accepts `*** Insert Before: PATH#Symbol` and `*** Insert After: PATH#Symbol` (also `PATH:N`), followed by `+` lines. Before inserts above the symbol's doc comments and attributes.
- `acus patch` accepts `*** Copy File: SRC` with `*** To: DST` to copy a file. The copy fails if DST exists, and later hunks in the same patch may update DST.
- A `context not found` error from `acus patch` ends with `nearest: path:A-B` and up to 8 lines of the most similar region, so the retry needs no search.
- `acus find` accepts `-c`/`--count` (per-file counts plus a total), `-o`/`--only-matching`, `-v`/`--invert-match`, `-m N`/`--max-count N` (per file; `--max-hits` stays the global cap), `-E` (a no-op), and `-P` (look-around and backreferences get a hint). `--include`, `--exclude` and `--exclude-dir` are mapped onto `-g`. All of them work inside `acus run` too. A grep-style `\|` is read as alternation, with a note on stderr, unless `-F`, `-E` or `-P` is given or the pattern already uses `|`.
- `acus show path:A,B` is the same as `path:A-B`.
- `acus ctx --tail N` keeps the last N lines plus every failure block, and `--grep RE` keeps the matching lines. The code they reference is computed from what is shown, and the full log is saved when lines were dropped.
- `acus guard` rule `ctx-pipe` refuses `acus ctx '…' | tail/head/grep` with the matching flag.

### Changed

- `acus patch` re-indents `+` lines when a hunk matched only after ignoring whitespace and indentation, and prints `note: tolerant match …`.
- `acus guard` build-pipe refusals give the exact rewrite: `cargo test -q | tail -n 40` becomes `acus ctx 'cargo test -q' --tail 40`, `head -n N` becomes `--max-output N`, and `grep PAT` or `Select-String` becomes `--grep`.
- `acus guard` no longer refuses `cat`, `head`, `tail`, `sed -n` or `Get-Content` reads, or Read tool calls, of files outside the session's repository.
- A trailing `/` in an exclude glob (`-g '!node_modules/'`) now works.

## [0.8.0]

### Added

- `acus guard` names the refused part of a compound command: the reason quotes the segment, says that no part of the command ran, and shows the acus command to run in its place (`acus find` with the grep flags carried over, `acus show path:A-B` for `head`/`sed -n`/`Get-Content -TotalCount`, `acus patch` with `*** Replace All: FILES` for `sed -i`), so the retry is one step.
- `acus guard` on Windows and PowerShell: `findstr` (cmd.exe and PowerShell, `/s /i /n /c:`), `type` and `more` reads, `grep.exe`-style and backslash command paths, `Get-ChildItem … | Select-String` pipelines (`gci`, `ls`, `dir`), `ls -r` and any `-Recurse` abbreviation as a recursive listing, `-creplace`/`-ireplace` and `.Replace()` edits written with `sc`, `Out-File`, `>` or `WriteAllText`, `ReadAllText` reads of source files, comma-separated `Get-Content a.rs,b.rs`, and `-Wait` followed like `tail -f`.
- `acus find` accepts `-r`, `-R` and `-H` as no-ops, like `-n`. Other grep and rg flags it lacks (`-c`, `-v`, `-o`, `-x`, `-m`, `--include`, `--exclude`) fail as before, now with a hint at the nearest `acus find` equivalent and `acus find --help`, also inside `acus run`.
- `acus patch --help` documents the patch format, including `*** Replace All: PATH [PATH...]` with its literal and regex blocks. `skill/SKILL.md` and the README describe it too.

### Changed

- `acus guard` no longer refuses `cat`, `head`, `tail`, `type` or `Get-Content` of `.json`, `.toml`, `.yaml` and `.yml` files, which agents read whole on purpose (the Read tool already leaves them alone). `sed -i` on them is still refused.
- `acus guard` no longer refuses `grep`, `rg`, `findstr` or `Select-String` on plain files that are not source (`.log`, `.txt`, `.csv`, data formats, `*.log` globs) or on stdin (`grep -c x <<< "$v"`, `grep -l x <(cmd)`). A directory, a glob, a variable, no operand with `rg`, `-r`, or a source file still counts as a code search.
- `acus guard` treats `cat a.rs >> all.txt` and `cat a.rs b.rs > all.txt` as copies, as before, but `2>&1` and `2>/dev/null` no longer make `cat a.rs 2>&1` look like one.
- `acus guard` also refuses `find … -exec grep` and `grep` inside `env`, `uv run` and `xargs` wrappers by their real command name.
- `acus patch` errors for a `-` or SEARCH line that holds only part of a line point at `*** Replace All: PATH` with a SEARCH or REGEX block in every case (before, only some). An unknown `***` directive lists every accepted form with its arguments and suggests the right one for a near miss (`*** Replace all:`, `*** Replace All:src`); a `<<<<<<< REGEX` block under `Update File` says it belongs under `Replace All`.

### Fixed

- `acus guard` parsed an escaped quote inside double quotes as the end of the string, which hid the commands after it; in PowerShell, backslash is a path character and the backtick the escape, so `ls src\; Select-String x a.rs` is two commands.

## [0.7.1]

### Changed

- `acus guard` also hints at edit scripts written to a file with `cat > x.py <<EOF` or `tee x.rb <<EOF` (`.py`, `.js`, `.mjs`, `.cjs`, `.rb`), not only at `python - <<EOF`. Writing a script file that does not rewrite a source file stays unhinted.

## [0.7.0]

### Added

- `acus log [REV] [PATH...] [-n N]`: one line per commit with date, author and changed lines, in agent, JSON and `run` output.
- `acus guard` hints at `acus diff` for plain `git status` and `git diff`, at `acus log` for `git log`, and at `acus diff REV^..REV` for `git show`, in Bash and PowerShell. Calls already compacted by a flag and `git show REV:path` pass without a hint.

## [0.6.1]

### Fixed

- Empty diffs now exit successfully; empty `find` and `diff` JSON results, including entries in `run --json`, emit `null`.

## [0.6.0]

### Added

- `acus update` replaces the binary in place with the latest GitHub release when it is newer, verified against the release's SHA-256, and reinstalls the skill; `--check` only reports, `--force` reinstalls. On by default as the `cmd-update` feature; it only goes online when called.

## [0.5.0]

### Added

- `acus guard` handles the Grep tool (refused with the equivalent `acus find`; counts and multiline searches pass), the Glob tool (a hint at `acus outline`) and the PowerShell tool on Windows (`Select-String`, `Get-Content`, builds piped into `Select-Object`, `-replace … | Set-Content`).
- `acus guard` logs every decision next to the config file, including calls run through the escape; `acus usage --guard` counts them per rule and lists the escaped calls.
- A trailing `# acus-skip` comment runs a refused shell command anyway, in Bash and PowerShell.
- `acus ctx` without a command runs `[ctx] command` or the project's tests, detected from Cargo.toml, Package.swift, go.mod, package.json (npm, pnpm, yarn or bun) or pytest files.
- `acus map [DIR]`: directory tree within a token budget. The largest directories open first, tests and docs last; single-child chains and single-file directories are rolled up; manifests tag project roots; small directories show their files' top-level symbols. `acus guard` hints at it for `tree`, `ls -R`, unfiltered `find` and `Get-ChildItem -Recurse`.
- `acus skill --install --hooks` registers the SessionStart and PreToolUse hooks in `~/.claude/settings.json`, keeping other settings and a backup.

### Changed

- `acus ctx` only points to the full log when output was cut or the command failed.
- `--json` output keeps keys in their natural order instead of sorting them.
- CI also runs on macOS.

### Fixed

- `acus ctx` on Windows passes the command to `cmd` verbatim, so quoted paths work.

## [0.4.0]

### Added

- `acus guard` refuses builds and tests (cargo, swift, go, npm/pnpm/yarn/bun, tsc, pytest, xcodebuild and others) piped into `tail`, `head` or `grep`, and suggests `acus ctx` with the same command.
- `acus guard` handles the Read tool: a Read without `offset`/`limit` of a source or Markdown file over 300 lines is refused with the file's outline. Add `Read` to the hook matcher (`"Bash|Read"`).
- `acus ctx` saves the full output to a temp file when it cuts or drops lines, and prints the path, so more of the output can be read without running the command again.

## [0.3.1]

### Added

- `acus guard` recognises Python, Node and Ruby scripts (heredoc, `-c`, `-e`) that read a source file, replace text and write it back. They still run, but the agent gets a hint to use `acus patch` next time.

## [0.3.0]

### Added

- `acus guard`, a Claude Code PreToolUse hook for Bash. It refuses `grep`/`rg` searches, `cat`/`head`/`tail`/`sed -n` reads of source files and `sed -i`/`perl -i` edits, and names the acus command to use instead. Pipe filters such as `git log | grep fix`, heredoc bodies and commands prefixed with `command ` pass.

## [0.2.0]

First release with prebuilt binaries for macOS, Windows and Linux.

### Added

- `acus skill` prints the bundled agent skill for a SessionStart hook; `acus skill --install` writes it to `~/.claude/skills/acus/` and `~/.agents/skills/acus/`.
- `install.sh` and `install.ps1` install the latest release and the skill.
- `find`: `-w`, `-l`, `-t TYPE`, `-u` and `-n`; long lines are clipped around the match.
- `patch`: `--fmt`, `Class.method` addresses, `Replace All` over directories and globs, better near-miss hints.
- `outline`/`show`: nested config keys, Markdown setext headings, Swift initialisers, decorators and doc comments belong to their symbol.
- `ctx` groups references per symbol and drops library frames; `usage --tools`.

### Changed

- `decide` warns and exits 2 without calling the API when no API key is set.
- `diff` compares unstaged changes against the index.
- `run` keeps going after a line that fails to parse.

### Fixed

- `find` no longer adds the regex hint to errors that are not regex errors, such as a missing path.
- `acus … | head` ends quietly instead of panicking on a closed pipe.

## [0.1.0]

Initial version: `find`, `outline`, `show`, `patch`, `ctx`, `diff`, `run`, `usage` and `decide`.

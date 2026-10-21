# Guard

`acus guard` is the PreToolUse hook that keeps agents on acus. Claude Code calls it before each Bash, PowerShell, Read, Grep and Glob call. It lets the call through, rewrites it into an acus call, or refuses it and names the acus command to use. Setup is in the [README](../README.md#agent-setup).

## Outcomes

- **Allow.** Calls with no acus equivalent pass. So do filters on other output (`git log | grep fix`), `grep` of stdin, searches and reads of logs, text, other plain files, and JSON, TOML and YAML data, `cat a.rs >> all.txt` copies and heredoc bodies. Output that feeds another command runs as written: `grep -l X | xargs …`, `cat f | python3 …`, `cat f | jq .`. So does a search redirected to a file, and `grep -q`. A display filter after a search or read (`grep X | head`, `| sort`) is not a consumer: the output reaches the agent, so the pipe is refused or rewritten as in the Rewrites table.
- **Captured output.** A segment whose stdout is data rather than output the agent reads runs unchanged, with no rewrite, refusal or hint: inside `$( )`, backticks, `<( )` or `>( )`; or with stdout redirected (`>`, `>>`, `1>`, `&>`, `>&2`, `>|`). Other segments of the same command still convert.
- **Loops and groups.** A segment inside `{ }`, `( )`, `if`, `for`, `while`, `until` or `case` runs unchanged, with no rewrite or refusal: acus output has headers and line numbers, and a loop runs its segment more than once. When its output still reaches the agent (not captured, redirected or piped), a read or search there gets a hint naming the acus command, such as `acus show 'a.rs:1-3'` for `head -3 a.rs` in a loop.
- **Parsers.** A command is parsed with a tree-sitter grammar, so quoting, heredocs, substitutions and groups are read as the shell reads them. The Bash tool uses tree-sitter-zsh when the hook process's `$SHELL` is `zsh` (Claude Code runs Bash-tool commands in the user's login shell, so zsh syntax such as `${(f)x}`, `=(cmd)`, `*(.)`, `repeat N { … }` and `|&` is read as written), and tree-sitter-bash otherwise. The PowerShell tool uses tree-sitter-powershell: pipelines, `>`, `>>`, `*>`, `Out-File` and `Set-Content` for output, `$( )`, `@( )`, `( )`, assignments and script blocks passed as arguments for captures, and `foreach`, `for`, `while` and `do` for loops. The grammars are the Cargo features `guard-zsh`, `guard-bash` and `guard-powershell`, all on by default (they enable the `acus-guard` crate's `zsh`, `bash` and `powershell` features). A build without one falls back to a simple hand-written splitter for that shell (Bash commands are then never rewritten); if it lacks Bash but has zsh, zsh reads Bash commands too.
- **Unparsed.** A Bash or zsh command the grammar rejects (an unterminated quote, zsh syntax that the Bash grammar does not know, glob qualifiers such as `*(om[1,3])` and `foreach x (…)` that the zsh grammar rejects) runs unchanged, as does one nested more than 48 brackets deep under zsh (the zsh grammar's scanner aborts the process on deeper nesting). A PowerShell line that the grammar rejects, such as `Get-Content a.rs,b.rs`, is split by the simple splitter instead.
- **Rewrite.** Bash in Claude Code only. When every offending segment has an exact replacement, the hook allows the call with `updatedInput` set to the acus command, so it costs no extra turn, and the agent's context shows what ran. Builds and tests are rewritten in every permission mode, but without a `permissionDecision`, so Claude Code's normal permission flow decides on the rewritten command.
- **Refuse.** One segment with no exact replacement refuses the whole command, so nothing in it runs. For a compound command, the reason names the refused segment, says that no part ran and shows the acus command. Codex hooks cannot rewrite, so they always refuse.
- **Hint.** The call runs and the reason suggests an acus command. Used for recursive listings, `git status`, `git diff` that `acus diff` cannot take, `log` and `show`, reads and searches in a loop or group, the Glob tool, and Python, Node or Ruby scripts that rewrite source with `.replace()` or `re.sub()`. A hint repeats on every call, so a rule's hint is shown on its first call in a session and then on every `hint_every`th call (`[guard] hint_every` in config.toml, default 10; `1` shows every one). The calls in between pass silently and are not logged. The state is `guard-hints/SESSION` next to guard.jsonl, and a SessionStart after a compaction or `/clear` starts the count again.

## Rewrites

| Command | Becomes |
| --- | --- |
| `cat FILE`, `cat -n FILE` | `acus show FILE` |
| `head FILE`, `head -n N FILE`, `head -N FILE`, `cat FILE \| head -N` | `acus show FILE:1-N` (N defaults to 10) |
| `tail -n N FILE`, `tail -N FILE` | `acus show FILE:-N` |
| `tail -n +N FILE`, `tail +N FILE`, `tail --lines=+N FILE`, `cat FILE \| tail -n +N` | `acus show FILE:N-` |
| `sed -n 'N,$p' FILE`, `sed -n "N,\$p" FILE` | `acus show FILE:N-` |
| `tail -n +N FILE \| head -M`, `cat FILE \| tail -n +N \| head -M` | `acus show FILE:N-(N+M-1)` (lines N to N+M-1) |
| `sed -n 'A,Bp;C,Dp;N' FILE`, each part `A,Bp` or `N` | `acus show FILE:A-B FILE:C-D FILE:N`; a regex address is refused |
| `grep`, `egrep`, `fgrep`, `rg` searches | `acus find`, with the flags below |
| `find DIR -name GLOB`, `find DIR -iname GLOB`, `rg --files [-g '!NAME'] [--hidden] [-u] [-L]` | `acus files DIR -g GLOB` |
| `BUILD \| grep RE [-A N] \| head -N`, `BUILD \| tail -N` | `acus ctx 'BUILD' --grep RE -A N --max-output N`, `acus ctx 'BUILD' --tail N` |
| `acus ctx 'CMD' \| tail -N`, `\| head -N`, `\| grep RE` | the same call with `--tail N` (the smaller of N and an existing `--tail M`), `--max-output N` or `--grep RE`; a filter with no exact flag is refused |
| `BUILD \| tail -N; echo "…${PIPESTATUS[0]}"` (or `${pipestatus[1]}`), the echo directly after the pipeline | the same `acus ctx` call, with `$?` in the echo |
| `git diff [REV\|A..B] [-- PATH…]`, with `--staged` or `--cached` | `acus diff -p [REV] [PATH…]`; `git diff --stat …` becomes `acus diff …`. `A...B`, several revisions and other flags keep the hint |
| `acus show\|outline\|find\|diff … \| head -N` | the same call with `--head N` |
| `cat FILE`, `tail -N FILE` or `grep` of a background task's `tasks/*.output` file | `acus ctx --log FILE` with the matching flags (`--tail N`, `--grep RE`) |
| `sed -i 's/OLD/NEW/g' FILE`, literal text, one regular file | `acus patch` with `*** Replace All: FILE`, left to the agent's permission prompt |
| `gh run view --log` or `--log-failed` piped through filters | `acus ctx 'gh run view …'` with the filters as flags |
| `timeout N BUILD \| filters` | `acus ctx 'BUILD' --timeout N` with the filters as flags; a timed build whose filters do not fold runs as written |

`cat`, `head` and `tail` convert only source and Markdown files. In a build, `VAR=x`, quotes and `time` are kept. A build with `$` or a backtick is refused, not rewritten, because `acus ctx` runs its text through a shell again: the suggestion shows the double-quoted `acus ctx "…"` form. Rewrites and suggestions keep `$VAR` and globs unquoted, so the shell expands them as it did in the original. `fd` is not converted; it gets a hint that names `acus files`.

## Search flags

These grep and rg flags map to `acus find`: `-i`, `-w`, `-x`, `-F`, `-l`, `-o`, `-v`, `-m N`, `-e`, `-t`, `-g`, `--hidden`, and the context flags `-A`, `-B`, `-C` (`-NUM` too). `grep -r` and `-n` are dropped, because acus recurses and always numbers lines. `grep -R` and `--dereference-recursive` become `-L`. `--include`, `--exclude` and `--exclude-dir` become `-g` globs; with plain names in braces (`--exclude-dir={a,b}`), one `-g` per name. `rg -L` and `--follow` become `-L`, and `rg -U` and `--multiline` become `-U`. `-c` and `--count` are not converted: a count is not the lines, so those commands run as written.

These are refused, because there is no exact equivalent: `-P`, `-z`, `-h`, grep's `-L`, `-U` and `--follow`, `-f FILE`, several `-e` patterns, a `-` for stdin, `ag` and `ack`, `rg -r` (replace), and combinations acus find cannot express, such as `-v` with `-o`, `-m` with `-v`, `-U` with `-v` or context, and `-x` with `-w`. A `sed -n` script with a regex address is refused too, and so is a `sed -n` range that is reversed or starts at 0.

## Listings, reads and edits

- `find DIR -name GLOB` and `rg --files` become `acus files`, unless `&&` or `||` follows (`acus files` exits 1 when nothing matches, `find` exits 0). `rg --files` with an include glob (`-g GLOB`, which overrides ignore rules in ripgrep) or `-t TYPE` (ripgrep's own type table) is not converted. `find` without `-type f` also lists matching directories; `acus files` lists files only. `find` without filters, `tree`, `ls -R` and `Get-ChildItem -Recurse` run with a hint at `acus map`.
- The Read tool, without `offset` or `limit`, is refused for a source or Markdown file over 300 lines, and the refusal carries the file's outline.
- `git status`, `git log` and `git show` run with a hint at `acus diff` or `acus log`. `git diff` becomes `acus diff` as in the Rewrites table, unless `&&` or `||` follows it, its output is piped, or its arguments are not ones `acus diff` takes; then it gets the hint. Flags that compact the output (`--short`, `--name-only`, `--format`) and `git show REV:path` pass without a hint. `git log --oneline` runs without a hint, and so do `git log` calls with `--format`, `--pretty`, `--stat`, `--shortstat` or `--name-only`; other `git log` calls get the hint.
- `sed -i 's/OLD/NEW/g' FILE` with literal text on one regular file is rewritten to an `acus patch` Replace All, left to the agent's permission prompt. Unlike sed, the patch fails and writes nothing when OLD is not found. Other `sed -i` forms (directories, symlinks, regex characters, other flags, several files) and `perl -i` are refused with a hint to use `acus patch`. `acus ctx ... | tail`, `| head` and `| grep` become ctx's own flags when one matches exactly. Any other filter on `acus ctx` output is refused, because the output was already filtered.
- PowerShell and Windows: the same rules apply to `Select-String` (`sls`), `findstr`, `Get-Content` (`gc`, `type`, `cat`, with `-Raw`, `-TotalCount` or `-Tail`), `Get-ChildItem … | Select-String`, `Select-Object -Last` after a build, `-replace` and `.Replace()` edits written with `Set-Content`, `sc`, `Out-File`, `>` or `WriteAllText`, and `ReadAllText` reads. PowerShell calls are refused, not rewritten. `;`, `&&` and `||` split commands in both PowerShell 5.1 and 7.

## Rule names

`grep`, `grep-tool`, `read-shell`, `read-tool`, `build-pipe`, `ctx-pipe`, `sed-i`, `tree`, `glob-tool`, `ps-replace`, `edit-script`, `git-status`, `git-diff`, `git-log`, `git-show`.

These names are written to `guard.jsonl` and read by `acus usage --guard`, so renaming one breaks the history.

## Escapes

- A refused command runs anyway with a `command ` prefix (Bash) or a trailing `# acus-skip` comment (both shells).
- Every decision, escapes included, is logged to `guard.jsonl` next to the config file. `acus usage --guard` counts rewrites, refusals, hints and escapes per rule and lists the escaped calls. An escape usually marks a refusal that acus could not replace.
- The hook command ends in `|| true`, so calls pass when acus is missing. `guard` can also be disabled in the configuration.

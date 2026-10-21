---
name: acus
description: Code search, reading and editing CLI that replaces grep/rg/find/cat/head/tail/sed and Read→Edit chains with one call. Use whenever about to search a codebase, read a file, symbol or line range, get an overview of a file or directory, edit one or more files (instead of sed -i, perl -pi or a Python replace script), run a build or tests and inspect the failures, or review uncommitted changes and recent commits.
---

# acus

One call answers what usually takes search → read → read. Output is line-numbered, grouped by enclosing symbol, and every truncation names the exact next command.

## Instead of … run

| Instead of | Run |
|---|---|
| `grep`, `rg`, `find` to locate code or usages | `acus find 'PAT' [PATH…] [-g '*.rs']`; `--block` adds the code around each hit; `-l`, `-w`, `-i`, `-c`, `-o`, `-v`, `-t py` work as in grep; `-A/-B/-C N` print N lines of context like grep (context lines are `N-`, hits `N`, groups split by `--`); `-x` whole lines, `-L` follows symlinks, `-U` multiline patterns; `-u --hidden` also searches ignored and hidden files |
| `sed -n A,Bp f \| grep 'x'` | `acus find 'x' f:A-B` (or `f#Sym`): searches only that range, with file-absolute line numbers; `f:-N` for the last N lines, `f:N-` from line N to the end |
| `grep 'x' log \| cut -c1-200 \| tail -30` | `acus find 'x' log --cut 200 --last 30`: `--cut N` clips each hit line to N chars, `--last N` keeps the last N hits per file |
| `ls -R`, `tree`, `find -type f` | `acus map [DIR]` |
| `find -name`, `fd`, `git ls-files`, Glob | `acus files [PATH…] [-g GLOB] [--hidden] [-u] [-t TYPE] [-L]`, e.g. `acus files src -g '*.rs'` |
| `wc -l` | `acus files --lines` (line count per file and a total; `--json` for scripts) |
| `cat`, `head`, `tail`, `sed -n`, or Read to look something up | `acus show src/a.rs#Parser::parse src/b.rs:40-80 README.md` (several in one call; `src/a.rs:-20` prints the last 20 lines, for `tail -20`; `src/a.rs:150-` prints line 150 to the end, for `tail -n +150`); `acus outline PATH… [--depth 1] [-k fn,struct]` for what a file or directory contains; `acus show REV:path#Sym` reads it as of a git revision; `--head N` stops after N printed lines on `show`, `outline`, `diff` and `find`, and `--cut N` clips long lines on `show`, `outline`, `diff` and `find` |
| `sed -i`, `perl -pi`, Python replace scripts, Edit | `acus patch` (below) |
| `cargo test \| tail`, `npm test 2>&1 \| grep …` | `acus ctx 'CMD'` with `--tail N` or `--grep RE` (`-A/-B/-C N` keep context lines, as in grep), `--cut N` clips long lines, and `--timeout SECS` (kills the command, keeps the output so far, exits 124); `acus ctx` alone runs the project's tests; `acus ctx --log FILE` reads a saved log, such as a background task's output file, through the same filters |
| `git status`, `git diff` | `acus diff [REV] [-p]` |
| `git blame` | `acus blame path#Sym` (or `path:A-B`): who last changed each run of lines |
| `grep` for who calls a function, or to see what it calls | `acus follow path#Sym [-d N] [--down] [--all]`: recursive callers, or callees with `--down`; only sure calls are followed, `?` lines are name matches to check by hand |
| `git log` | `acus log [REV] [PATH…] [-n N]`; one commit's changes: `acus diff REV^..REV` |

A hook catches the commands on the left: it rewrites those with an exact acus equivalent and refuses the rest, and a refused command costs a turn. Calling acus directly is also shorter, because line numbers and enclosing symbols come with the first result.

## Batch lookups

Every tool call is one turn. Before calling, list the lookups you still need, then send all that do not depend on each other at once: several tool calls in one response, or one `acus run` with one command per line:

```bash
acus run <<'EOF'
find 'fn parse' --block
show src/a.rs#f src/b.rs:10-30
outline src/lib.rs
EOF
```

## Patch

Edit with `acus patch`, with the semantics of Claude's Edit tool. No prior Read needed. Put every change of one step into one patch. Inside a compound command (after `cd DIR &&`), write the patch to a file and run `acus patch --file F`, because some agent shells refuse heredocs there.

```bash
acus patch <<'PATCH'
*** Begin Patch
*** Update File: src/lib.rs
<<<<<<< SEARCH
        let x = 1;
=======
        let x = 2;
>>>>>>> REPLACE
<<<<<<< SEARCH
self.done = false;
=======
self.done = true;
>>>>>>> REPLACE
*** Replace Symbol: src/lib.rs#Parser::reset
+    fn reset(&mut self) {
+        self.pos = 0;
+    }
*** Add File: notes.md
+hello
*** Delete File: old.rs
*** End Patch
PATCH
```

Rules for SEARCH/REPLACE blocks under `*** Update File:`:
- Copy the old text exactly, without the line-number prefix `acus show` prints. It is a substring: it may be part of a line or span lines. If it is not found as written, acus also accepts it with those prefixes stripped, ignoring trailing whitespace or ignoring leading indentation (tabs vs spaces; REPLACE then takes the file's indentation), and prints a `note:`.
- It must occur exactly once. With no match the error shows the nearest lines; with several it lists their line numbers. Add a neighbouring line to make it unique.
- For every occurrence, across files, or with a regex, use `*** Replace All:` (below).
- `+` prefixes only the new lines of Replace Symbol, Insert Before/After and Add File; SEARCH/REPLACE text has none.
- A file whose syntax (tree-sitter) gets more errors fails the whole patch and names the line; fix the block, or pass `--allow-broken` to write anyway.
- A multi-line SEARCH matches a CRLF file as LF text and prints a `note:`; the file stays CRLF.

Output lists the written lines with their new numbers, so no re-read is needed. All or nothing: on error nothing was written; fix the block from the lines shown and resend the whole patch. `--check` validates without writing and reports what would change; `--diff` prints a unified diff (`--check --diff` previews). `--fmt` cannot be combined with `--check`. `--fmt` runs the file's formatter (rustfmt, ruff/black, prettier, swift-format, gofmt) over the whole file and prints the formatted lines, so the next patch's context still matches; add it only where the project requires formatted code, since it also changes lines you did not edit. Pick a heredoc delimiter that no line of the patch equals (`PATCH`, or `ACUS_END` when editing docs or scripts). Symbols may be written `Class.method` too.

More directives, mixable with the above in one patch (`acus patch --help` has the full syntax):

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
*** Delete Symbol: src/a.rs#unused
*** Move Symbol: src/a.rs#helper
*** After: src/b.rs#main
*** Insert After: src/a.rs#main
+fn helper() {}
*** Copy File: src/a.rs
*** To: src/a_copy.rs
```

- `*** Replace All: PATH [PATH...]` takes space-separated files, directories and globs (`*.py`, matched below the current directory) and changes every occurrence. SEARCH blocks are literal and whitespace-exact and may span lines; REGEX blocks are Rust regex run per file with `(?m)`, with `${1}` in the replacement. A block that matches nothing fails the patch.
- `Delete Symbol` and `Move Symbol` take doc comments and attributes along; `Move Symbol` needs `*** Before:` or `*** After: path#Sym`, or `*** To: path`.

## Reading the output

- Exit 0 = results, 1 = nothing found (not a failure), 2 = error plus a `hint:` line with the fix; `ctx` returns the command's own code.
- `… N more …` names the follow-up command; run it instead of searching again.
- Addresses from earlier output (`path#Sym`, `path:A-B`) can be pasted verbatim; a unique suffix such as `#parse` is enough, and an ambiguous one lists the candidates. Config keys nest with `::`: `cfg.yaml#server::port`.

## Common mistakes

- Unquoted globs are expanded by the shell: write `-g '*.ts'`, and `-g '!dist'` to exclude.
- Patterns are Rust regex, so `foo(` fails; use `-F` for literal text.
- Symbols exist for Rust, Swift, TS/JS, Svelte, Python, Shell, C, CSS, SCSS, HTML, XML/plist, Markdown and TOML/JSON/YAML keys. Other files: `find` and `show path:A-B`.
- Paths may be relative, absolute or start with `~`.

Flags: `acus help <command>`.

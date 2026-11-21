---
name: acus
description: Code search, read, patch and git-summary CLI for coding agents. Always invoke this skill first, before any Grep, Glob, Read or Edit call and before any Bash grep, rg, find, cat, head, sed -i, git diff, git log or ls. It owns every code lookup and edit, so use it to find usages, callers or definitions (acus find, follow), read lines N-M or a function (acus show), list files with line counts (acus files), see what changed on a branch or versus main (acus diff main -p), read git log for a path (acus log), rename or replace text across files, add, change or delete a method or lines (acus patch), search a log file and keep the last hits, run cargo test or npm test and show only failures (acus run), and chain several dependent lookups into one `acus script` or run a saved script by name. Results are line-numbered and grouped by symbol.
---

# acus

One call replaces search → read: line-numbered, grouped by symbol; every truncation names the next command.

## Instead of … run

| Instead of | Run |
|---|---|
| grep, rg, find | `acus find 'PAT' [PATH…]`; `--block` adds code |
| cat, head, tail, sed -n, Read | `acus show PATH#Sym PATH:A-B`; `acus outline PATH` |
| Glob, find -name, ls, fd, git ls-files | `acus files [PATH…] [-g GLOB]`; `--lines` adds counts |
| ls -R, tree | `acus map [DIR…]` |
| sed -i, perl -pi, Edit | `acus patch` (below) |
| cargo test, npm test | `acus run 'CMD'`; failures plus their code |
| git status, git diff | `acus diff [REV]`; `-p` adds the changed lines |
| git log | `acus log [-n N] [REV] [PATH…]` |
| git blame | `acus blame PATH#Sym` |
| who calls, what it calls | `acus follow PATH#Sym`; `--down` for callees |
| several dependent lookups, a survey | `acus script <<'JS' … JS` |
| a script to keep and run again | `acus script save NAME --note TEXT`; `acus script NAME key=value`; `acus script list` |
| edit, then test or build | `acus patch --run 'CMD'` (below) |

```bash
acus script <<'JS'
const hits = await find('fn parse', {glob: '*.rs'});
return await show(hits.head(2).map((h) => `${h.path}:${h.line}-${h.line + 20}`));
JS
```

Use a script when the next call is known before the first one runs; when a step depends on what you read, make single calls. Saved scripts are ES modules with `description`, `params` and a default function; `save` needs `--note`. Details: [script](reference/script.md).

Exact forms:

| Task | Command |
|---|---|
| callers, callees | `acus follow src/parser.rs#Parser::parse`, add `--down` for callees |
| read lines 40-80 | `acus show src/parser.rs:40-80` |
| branch vs main, changed lines | `acus diff main -p` |
| rename everywhere under src | `*** Replace All: src` block in `acus patch` |
| delete lines 40-44 | `*** Delete Lines: src/a.rs:40-44` plus `*** Expect:` in `acus patch` |
| edit, then run the tests | `acus patch --run 'cargo test' <<'PATCH' … PATCH` |
| run saved script `fix-imports` | `acus script fix-imports module=./util` |
| find, then show, then callers | one `acus script`; never chain acus commands with `&&` |

```bash
acus patch <<'PATCH'
*** Begin Patch
*** Replace All: src
<<<<<<< SEARCH
old_timeout
=======
timeout
>>>>>>> REPLACE
*** End Patch
PATCH
```

```bash
acus script <<'JS'
const hits = await find('fn parse_config', {glob: '*.rs'});
const addr = hits.head(1).map((h) => `${h.path}#parse_config`);
return [await show(addr), await follow(addr[0])];
JS
```

Flags beyond each default: [find](reference/find.md), [read](reference/read.md), [patch](reference/patch.md), [git](reference/git.md), [run](reference/run.md), [script](reference/script.md). `acus help <command>` lists them too.

## Patch

Edit with `acus patch`; no prior read needed.

The heredoc is the normal form, also after `cd DIR &&`.

```bash
acus patch <<'PATCH'
*** Begin Patch
*** Update File: src/lib.rs
<<<<<<< SEARCH
        let x = 1;
=======
        let x = 2;
>>>>>>> REPLACE
*** End Patch
PATCH
```

- SEARCH is the exact text without `acus show` line numbers; it must occur once; add a line when ambiguous. A miss shows nearest lines.
- All or nothing: reports every failing block; fix all, resend once.
- `acus patch -q`: summary only.
- Output shows the written lines; don't re-read them.
- One patch holds the whole change set; never write files with `cat >` or Python scripts.
- `acus patch --check` validates a large patch without writing; `--diff` shows the result.
- `acus patch --run 'CMD'` runs CMD after the patch applies, with `acus run`'s output and exit code; `--check` runs nothing.
- `acus patch --fmt` formats the written files; add it only where the project requires it.
- Symbol edits, `*** Append File`, line directives, `*** Resolve Conflict`, `*** Replace All` and `-q` are in [patch](reference/patch.md).

## Addresses

An address names code without reading it first; every command that takes a path accepts one, and addresses printed earlier can be pasted back.

- `PATH:A-B` is a line range, `PATH:A` one line, `PATH:-N` the last N lines, `PATH:N-` line N to the end, and `PATH#Sym` a symbol. A unique suffix such as `#parse` is enough; an ambiguous one lists the candidates. Config keys nest with `::`: `cfg.yaml#server::port`.
- Paths are relative to the current directory, not the repository root, unless the output prints them from there. `REV:PATH` reads a file as of a commit, branch or tag, counted from the repository root as git does.

## Reading the output

- Exit 0 = results, 1 = nothing found (not a failure), 2 = error with a `hint:` line naming the fix; `run` returns the command's own code.
- Limit output with `--head N` (`find`, `show`, `outline`, `diff`) or `acus run --tail N`, `--grep RE`, `--drop RE`.
- `… N more …` names the follow-up command; run it instead of searching again.
- `run` says `3rd identical failure …` after three identical failures in one session; fix the key line it repeats instead of rerunning.
- `acus show` on a file over 400 lines prints its outline, and output stops near 24k characters; `--full` prints everything. Details: [read](reference/read.md).

## Common mistakes

- A SEARCH block that differs from the file only in whitespace (tabs vs spaces, extra spaces or blank lines) misses: add `--fuzzy` after the exact forms fail; it still refuses two matches.
- A pattern that starts with `-` is read as a flag: use `acus find -e '-x' PATH`.
- Pipes: `| head -N` after `find`, `show`, `outline` or `diff` is `--head N`; `acus run --tail`, `--grep` and `--drop` replace `| tail` and `| grep`.
- `-A` and `-C` print grep-style context; `--block` prints the enclosing symbol, usually what you want.
- `PATH:A:B` is not a range and fails; the range is `PATH:A-B`.
- `*** Add File` refuses an existing file; use `*** Update File`, `*** Append File` or `*** Replace All`.
- Unquoted globs are expanded by the shell: write `-g '*.ts'`, and `-g '!dist'` to exclude.
- Patterns are Rust regex, so `foo(` fails; use `-F` for literal text.
- A script call without `await` returns a promise, not the result: write `await find(...)`.
- Symbols exist for Rust, Swift, TS/JS, Svelte, Python, Shell, C, CSS, SCSS, HTML, XML/plist, Markdown and TOML/JSON/YAML keys; on other files use `find` and `show PATH:A-B`.

This covers everyday use; invoke the acus skill only for a flag or form not shown here. More: `acus self skill --full`, `--topic <topic>`.

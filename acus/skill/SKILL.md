---
name: acus
description: Code search, read, patch and git-summary CLI for coding agents. Replaces grep, rg, find, Glob, cat, head, tail, Read, sed -i, perl -pi, Python replace scripts and Edit to search a codebase, read a file, symbol or line range, or edit files, and replaces git status, git diff, git log and git blame to review changes. Also runs builds and tests and shows only their failures. Results are line-numbered and grouped by enclosing symbol, every truncation names the next command, and patches apply all or nothing.
---

# acus

One call answers what usually takes search → read → read. Output is line-numbered, grouped by enclosing symbol, and every truncation names the exact next command.

## Instead of … run

| Instead of | Run |
|---|---|
| grep, rg, find to locate code or usages | `acus find 'PAT' [PATH…]`; `--block` adds the code |
| grep on a range | `acus find 'PAT' PATH:A-B` or `PATH#Sym` |
| cat, head, tail, sed -n, Read | `acus show PATH#Sym PATH:A-B`; `acus outline PATH` |
| ls -R, tree | `acus map [DIR…]` |
| find -name, fd, git ls-files, Glob | `acus files [PATH…] [-g GLOB]`; `--lines` counts lines |
| sed -i, perl -pi, Python replace, Edit | `acus patch` (below) |
| cargo test, npm test | `acus ctx 'CMD'`; failures plus the code they name |
| git status, git diff | `acus diff [REV]`; `-p` adds the changed lines |
| git log | `acus log [REV] [PATH…]` |
| git blame | `acus blame PATH#Sym` |
| who calls, or what a symbol calls | `acus follow PATH#Sym`, `--down` for callees |

Flags beyond each default: [find](reference/find.md), [read and addresses](reference/read.md),
[patch](reference/patch.md), [diff and log](reference/git.md), [build output](reference/ctx.md).
`acus help <command>` lists them too, and `acus self skill --topic <topic>` prints a reference.

## Patch

Edit with `acus patch`; it needs no prior read and applies all or nothing.

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
*** Replace Symbol: src/lib.rs#Parser::reset
+    fn reset(&mut self) {
+        self.pos = 0;
+    }
*** Add File: notes.md
+hello
*** End Patch
PATCH
```

- SEARCH is the exact old text, without the `acus show` line-number prefix; it must occur exactly once, so add a neighbouring line when it is ambiguous. A miss shows the nearest lines.
- The whole patch is all or nothing and reports every failing block; fix all of them and resend once.
- `acus patch --check` validates a large patch without writing, and `acus patch --check --diff` shows the result. `acus patch --fmt` formats the written files; add it only where the project requires it.
- `acus patch -q` prints only the summary, so use it when the written lines are not needed.
- `*** Append File`, `*** Delete Lines` and `*** Move Lines` (both with `*** Expect:`), `*** Resolve Conflict`, symbol moves and `*** Replace All` are in [patch](reference/patch.md).

## Addresses

An address names code without reading it first. Every command that takes a path accepts one, and addresses printed earlier can be pasted back.

```bash
acus show src/parser.rs#Parser::parse src/parser.rs:40-80 src/parser.rs:-20 README.md
acus find 'todo' src/parser.rs:40-80 src/parser.rs#Parser
acus blame src/parser.rs#Parser::parse
```

- `PATH:A-B` is a line range, `PATH:A` one line, `PATH:-N` the last N lines, `PATH:N-` line N to the end, and `PATH#Sym` a symbol. A unique suffix such as `#parse` is enough; an ambiguous one lists the candidates. Config keys nest with `::`: `cfg.yaml#server::port`.
- Paths are relative to the current directory. `REV:PATH` reads a file as of a commit, branch or tag, counted from the repository root as git does.

## Batch lookups

Batch independent lookups in one `acus run`.

```bash
acus run <<'EOF'
find 'fn parse' --block
show src/a.rs#f src/b.rs:10-30
outline src/lib.rs
EOF
```

`acus run` exits 0 when at least one line succeeded and lists the rest in a `failed: N of M lines` footer; it exits 2 only when every line failed or `--fail-fast` stopped at the first. Output is cut within 24k characters, and each cut names the follow-up with `--head N` (or `acus run --full`).

## Reading the output

- Exit 0 = results, 1 = nothing found (not a failure), 2 = error with a `hint:` line naming the fix; `ctx` returns the command's own code.
- `… N more …` names the follow-up command; run it instead of searching again.
- `acus show` prints a whole file over 400 lines as its outline, first 60 lines and pointers, and stops a call near 24k characters; `--full` prints everything, `--head N` stops after N printed lines, and `--cut N` clips long lines. The two caps work on `find`, `outline`, `diff` and the other read commands.

## Common mistakes

- A SEARCH block that is not found: read every block the error reports, fix them all and resend once. Use `--check` first on a large patch; an error writes nothing.
- A pattern that starts with `-` is read as a flag: use `acus find -e '-x' PATH`.
- Piping to `head` or `tail` throws work away and the hook rewrites it: use `--head N`, `--tail N` or `-q`.
- `-A` and `-C` print grep-style context; `--block` prints the enclosing symbol, which is usually what you want.
- `PATH:A:B` is not a range and fails; the range is `PATH:A-B`.
- Paths are relative to the current directory, not the repository root, unless the output prints them from there.
- `*** Add File` refuses an existing file; use `*** Update File`, `*** Append File` or `*** Replace All`.
- Unquoted globs are expanded by the shell: write `-g '*.ts'`, and `-g '!dist'` to exclude.
- Patterns are Rust regex, so `foo(` fails; use `-F` for literal text.
- Symbols exist for Rust, Swift, TS/JS, Svelte, Python, Shell, C, CSS, SCSS, HTML, XML/plist, Markdown and TOML/JSON/YAML keys; on other files use `find` and `show PATH:A-B`.

More flags: `acus self skill --full` and `acus self skill --topic <topic>`.

# acus

Use acus instead of shell tools. A hook rewrites or refuses grep, rg, find, cat, sed, head, tail and piped builds.

| Instead of | Run |
|---|---|
| grep, rg | `acus find 'PAT' [PATH…]` (`--block`, `-l`, `-w`, `-i`, `-F`, `-g '*.rs'`, `-g '!dist'` excludes); `acus find 'PAT' a.rs#Sym b.rs:40-80` searches only there |
| find -name, fd, git ls-files, Glob, wc -l | `acus files [PATH…] -g '*.rs' [--lines] [--depth=N] [--head=N] [--cut=N]` |
| cat, sed -n, head, tail, Read, git show | `acus show a.rs#Sym b.rs:40-80 c.rs:-N c.rs:N- REV:a.rs#Sym`; `acus outline PATH` |
| who calls / what it calls | `acus follow a.rs#Sym [--down]` |
| ls -R, tree | `acus map [DIR…]` |
| sed -i, Edit | `acus patch` (below) |
| `cargo test \| tail` | `acus ctx 'CMD' [--tail N] [--grep RE]`; `acus ctx --log FILE` for a saved log |
| git status/diff/log | `acus diff [-p]` (`--stat`, `--name-only`, `--conflicts`), `acus log` |

Batch independent lookups: several tool calls, or one `acus run`.

```bash
acus patch <<'PATCH'
*** Begin Patch
*** Update File: src/lib.rs
<<<<<<< SEARCH
exact old text, occurring once
=======
new text
>>>>>>> REPLACE
*** End Patch
PATCH
```

One patch per step. Add `-q` (summary only) unless you need the written lines. `*** Delete Lines: a.rs:40-44` needs `*** Expect:` with line 40's text. `--fmt` only where the project requires it (it reformats the whole file). All directives and rules: `acus self skill --full`.

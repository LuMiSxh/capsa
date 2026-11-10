# acus

One call replaces search → read: line-numbered, grouped by symbol; every truncation names the next command.

| Instead of | Run |
|---|---|
| grep, rg, find | `acus find 'PAT' [PATH…]`; `--block` adds code |
| cat, head, tail, sed -n, Read | `acus show PATH#Sym PATH:A-B`; `acus outline PATH` |
| Glob, find -name, ls, fd, git ls-files | `acus files [PATH…] [-g GLOB]`; `--lines` adds counts |
| sed -i, perl -pi, Edit | `acus patch` (below) |
| cargo test, npm test | `acus ctx 'CMD'`; failures plus their code |
| git status, git diff | `acus diff [REV]`; `-p` adds the changed lines |
| git log | `acus log [-n N] [REV] [PATH…]` |
| who calls, what it calls | `acus follow PATH#Sym`; `--down` for callees |

- Don't pipe: `--head N` (`find`, `show`, `outline`, `diff`); `acus ctx --tail/--grep/--drop`; the guard rewrites it anyway.

Batch lookups with `acus run`.

Edit with `acus patch`; no prior read, all or nothing.

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

- SEARCH is the exact text without `acus show` line numbers; it must occur once, so add a line when ambiguous. A miss shows nearest lines.
- All or nothing: reports every failing block; fix all, resend once.
- Output shows the written lines with new numbers; don't re-read them.
- One patch holds the whole change set; never write files with `cat >`, `printf >` or Python scripts.
- `acus patch -q`: summary only.

More: `acus self skill --full`, `--topic <topic>`.

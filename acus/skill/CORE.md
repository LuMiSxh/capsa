# acus

One call replaces search → read: line-numbered, grouped by symbol; every truncation names the next command.

| Instead of | Run |
|---|---|
| grep, rg, find | `acus find 'PAT' [PATH…]`; `--block` adds code |
| cat, head, tail, sed -n, Read | `acus show PATH#Sym PATH:A-B`; `acus outline PATH` |
| Glob, find -name, ls, fd, git ls-files | `acus files [PATH…] [-g GLOB]`; `--lines` adds counts |
| sed -i, perl -pi, Edit | `acus patch` (below) |
| cargo test, npm test | `acus run 'CMD'`; failures plus their code |
| git status, git diff | `acus diff [REV]`; `-p` adds the changed lines |
| git log | `acus log [-n N] [REV] [PATH…]` |
| who calls, what it calls | `acus follow PATH#Sym`; `--down` for callees |
| several dependent lookups, a survey | `acus script <<'JS' … JS` |

```bash
acus script <<'JS'
const hits = await find('fn parse', {glob: '*.rs'});
return await show(hits.head(2).map((h) => `${h.path}:${h.line}-${h.line + 20}`));
JS
```

- Limit output with `--head N` (`find`, `show`, `outline`, `diff`) or `acus run --tail N`, `--grep RE`, `--drop RE`.

Edit with `acus patch`; no prior read needed.

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
- Output shows the written lines; don't re-read them.
- One patch holds the whole change set; never write files with `cat >` or Python scripts.
- `acus patch -q`: summary only.

This covers everyday use; invoke the acus skill only for a flag or form not shown here. More: `acus self skill --full`, `--topic <topic>`.

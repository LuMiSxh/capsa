# acus

One call answers what usually takes search → read → read. Output is line-numbered, grouped by enclosing symbol, and every truncation names the exact next command.

| Instead of | Run |
|---|---|
| grep, rg, find to locate code or usages | `acus find 'PAT' [PATH…]`; `--block` adds the code |
| cat, head, tail, sed -n, Read | `acus show PATH#Sym PATH:A-B`; `acus outline PATH` |
| sed -i, perl -pi, Python replace, Edit | `acus patch` (below) |
| cargo test, npm test | `acus ctx 'CMD'`; failures plus the code they name |
| git status, git diff | `acus diff [REV]`; `-p` adds the changed lines |

Batch independent lookups in one `acus run`.

Edit with `acus patch`; it needs no prior read and applies all or nothing.

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

- SEARCH is the exact old text, without the `acus show` line-number prefix; it must occur exactly once, so add a neighbouring line when it is ambiguous. A miss shows the nearest lines.
- The whole patch is all or nothing and reports every failing block; fix all of them and resend once.
- `acus patch -q` prints only the summary, so use it when the written lines are not needed.

More flags: `acus self skill --full` and `acus self skill --topic <topic>`.

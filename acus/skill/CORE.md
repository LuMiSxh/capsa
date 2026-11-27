# acus

```bash
acus script <<'JS'
await edit('src/config.rs', 'timeout = 30', 'timeout = 60');
await run('cargo test', {tail: 40})
JS
```

| Instead of                                               | Run                                                                                    |
| -------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| independent acus commands, no logic                      | `acus batch <<'EOF'`: one acus command per line, in parallel, no JS                    |
| grep, rg, find                                           | `acus find 'PAT' [PATH…]`; `--block` adds code                                         |
| cat, head, tail, sed -n, Read                            | `acus show a.rs#A b.rs:40-80 c.rs#B`: several addresses, one call; `acus outline PATH` |
| Glob, find -name, fd, git ls-files                       | `acus files [PATH…] [-g GLOB]`; `--lines` counts                                       |
| ls -R, tree                                              | `acus map [DIR]`                                                                       |
| sed -i, perl -pi, Edit                                   | `acus patch`                                                                           |
| cargo test, npm test                                     | `acus run 'CMD'`; failures with code                                                   |
| git status, git diff                                     | `acus diff [REV]`; `-p` adds changed lines                                             |
| git log                                                  | `acus log [-n N] [REV] [PATH…]`                                                        |
| callers, callees                                         | `acus follow PATH#Sym`; `--down` for callees                                           |

- Last expression prints unless the script calls `return` or `print`; `read`, `write`, `edit` and `fetch` replace Read, Write, Edit and WebFetch under the same permissions; `--check` only parses.
- A script used twice: `acus script save NAME`; run `acus script NAME key=value`.
- Cap every call: `find --max-hits 20`, `--block --head 80`, `outline --max-lines 200`, `show --head N`, `diff --stat` before `-p`; raise only to edit.
- Limit `acus run` output: `--tail N`, `--grep RE`, `--drop RE`.
- No prose before a tool call unless required.

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

- SEARCH is exact text without `show` numbers, once only; add context if ambiguous; a miss shows nearest lines.
- All or nothing: reports every failing block; fix all, resend once.
- A SEARCH block that fails twice: `acus show` the symbol, re-quote from output; `--fuzzy` only after exact matching fails.
- `acus patch --run 'CMD'` runs CMD after the edit, as `acus run`; `--check` validates a large patch without writing.
- Written lines print; don't re-read. `-q` for summary only.
- One patch holds the whole change; never `cat >` or Python writes.

More: `acus self skill --full`, `--topic <topic>`.

# diff, log, blame and follow

## diff

```bash
acus diff [REV] [-p] [--stat | --name-only | --conflicts]
acus diff REV^..REV -- PATH
```

- Without REV, a header names the branch, upstream ahead/behind, staged, unstaged, untracked and
  conflict counts, and any merge, rebase or cherry-pick in progress.
- Symbol groups by default; `-p` adds the changed lines, summarises a moved symbol once
  (`moved … ← path:range`), outlines an added file, hides blank changed lines and caps each file
  at 80 changed lines. `--full` restores full bodies and every line.
- `--stat` prints `path +A -R` per file and a total; `--name-only` one `STATUS path` line each;
  `--conflicts` lists unmerged paths with line ranges and exits 1 when there are none.
- `--staged` shows the index against the revision. `--max-lines` caps output (default 300);
  `--head` and `--cut` cap lines.
- `--json` is `{"header": …, "files": […]}`; the header is absent for `REV..REV` and an empty
  diff is `{"files": []}`.

## log

```bash
acus log [REV] [PATH…] [-n N]
```

One `hash subject (age)` line per commit; `-n` sets the count (default 15), and `--head` and
`--cut` cap output. One commit's changes: `acus diff REV^..REV`.

## blame

```bash
acus blame PATH#Sym
```

Who last changed each run of lines of a symbol or `PATH:A-B` range, one line per commit. It
reads the working tree, so `REV:PATH` is not accepted.

## follow

```bash
acus follow PATH#Sym [-d N] [--down] [--all]
```

Recursive callers, or callees with `--down`; `-d` sets the depth (default 2). Only sure calls
are followed; `?` lines are name matches to check by hand. `--all` lifts the caps.

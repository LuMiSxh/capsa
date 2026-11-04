# find

Searches with a Rust regex, groups hits by file and enclosing symbol, and prints file-absolute
line numbers. With no path it searches piped stdin, else the current directory.

```bash
acus find 'PAT' [PATH…] [-g '*.rs'] [-i] [-F] [--block]
```

## Pattern

- `-e PAT` (repeatable) when the pattern starts with `-` or more than one pattern is needed.
- Patterns are Rust regex: escape `(`, `[` and `.`; use `-F` for literal text and `-U` to match
  across lines. `-E` and `-P` are accepted for grep habits and change nothing.
- `-i` ignores case, `-w` matches whole words, `-x` whole lines, `-v` prints lines that do not
  match, and `-o` only the matched text.

## Paths and filters

- `PATH:A-B`, `PATH:A`, `PATH:-N`, `PATH:N-` and `PATH#Sym` search only that range or symbol.
- `-g GLOB` includes and `-g '!GLOB'` excludes; `--include`, `--exclude` and `--exclude-dir` do
  the same for grep habits. Quote globs so the shell does not expand them.
- `-t rs,py` limits by file type, `--hidden` adds hidden files, `-u` ignores `.gitignore`, and
  `-L` follows symlinks.
- `-` reads stdin; a pipe is read to EOF and a note names the wait after 2 s of silence. A
  terminal or empty stdin walks the current directory.

## Output

- `--block` prints the enclosing symbol instead of the matching line; `-b` is its short form.
- `-A N`, `-B N` and `-C N` print grep-style context (`N-` marks context, `--` splits groups).
- `-l` lists matching files, `-c` counts matching lines per file, `-m N` stops after N matching
  lines per file, and `--last N` keeps the last N hits per file.
- `--max-hits N` caps the total (default 50) and `--max-lines N` caps one block (default 80).
- `--cut N` clips each hit to N characters, `--head N` stops after N printed lines, and
  `--no-syntax` skips tree-sitter.
- `--json` for scripts; `--human` for tables and colour.

Exit 1 means no match, not a failure.

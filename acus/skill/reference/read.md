# read and addresses

## show

```bash
acus show PATH#Sym PATH:A-B PATH [--context N] [--full]
acus show REV:PATH#Sym
```

- Several addresses in one call; an address may be a whole path.
- `PATH:A-B` is a line range, `PATH:A` one line, `PATH:-N` the last N lines, `PATH:N-` line N to
  the end, and `PATH#Sym` a symbol. A unique suffix such as `#parse` is enough; an ambiguous one
  lists the candidates. Config keys nest with `::`: `cfg.yaml#server::port`.
- `--context N` adds lines around symbol and range targets.
- A whole file over 400 lines prints its outline, first 60 lines and pointers; a call stops near
  24k characters naming the address that shows the rest. `--full` prints everything, `--head N`
  stops after N printed lines, `--cut N` clips long lines, and `--json` is never capped.
- `REV:PATH` reads a file as of a commit, branch or tag, counted from the repository root;
  `REV:./PATH` counts from the current directory.

## outline

```bash
acus outline PATH… [--depth 1] [-k fn,struct] [-g '*.rs']
```

`--depth N` keeps symbols nested less than N deep, `-k` keeps the named kinds and their parents,
`-g` filters a directory walk, and `--max-lines N` caps the output (default 400).

Symbols exist for Rust, Swift, TS/JS, Svelte, Python, Shell, C, CSS, SCSS, HTML, XML/plist,
Markdown and TOML/JSON/YAML keys. Other files: use `find` and `show PATH:A-B`.

## files

```bash
acus files [PATH…] [-g '*.rs'] [--lines] [--depth N]
```

`--lines` adds each file's line count and a total, `--depth N` limits the walk, `--hidden` and
`-u` work as in `find`, `-t` limits by type, `-L` follows symlinks, and `--head` and `--cut`
cap output.

## map

```bash
acus map [DIR…] [--depth N] [--budget TOKENS]
```

Maps several paths as one tree; `--budget` (default 400 tokens) opens the largest directories
first, `--depth` limits the levels, and `--head` and `--cut` cap output.

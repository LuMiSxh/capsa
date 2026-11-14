#!/usr/bin/env bash
# Rebuilds a golden guard corpus from the hook's log: crates/acus-guard/src/tests/corpus.jsonl
# (macOS, the default) or corpus-windows.jsonl (--windows).
#
# Usage: scripts/guard-corpus.sh [--windows] [GUARD_JSONL]
#   GUARD_JSONL defaults to "$HOME/.local/state/acus/guard.jsonl", or with
#   --windows to "$LOCALAPPDATA/acus/state/guard.jsonl".
#   GUARD_CORPUS_EXCLUDE names a file of private project names, one per line, kept outside the
#   repository (default "$HOME/.config/acus/guard-corpus-exclude.txt"). The script refuses to run
#   without it, so those names never reach this public repository.
#   GUARD_CORPUS_REDACT names an optional file of "name=placeholder" lines (default
#   "$HOME/.config/acus/guard-corpus-redact.txt"). Names are replaced case-insensitively in
#   commands and cwds before the filters. PYTHON picks the interpreter (default python3, else python).
# Each case is {"tool","cwd","cmd"} plus a "dialect" ("zsh", "powershell"; "bash" when
# absent): at most 15 logged calls per (dialect, kind, rule), spread evenly over
# the log's order, then the hand-written cases below. After a change, rerun it and review the
# golden snapshot with `cargo insta review`. The --windows corpus is public: falsify its private names
# (consistent invented names, same command shape) before committing.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
windows=0
if [ "${1:-}" = "--windows" ]; then
  windows=1
  shift
fi
if [ "$windows" = 1 ]; then
  log=${1:-"${LOCALAPPDATA:?LOCALAPPDATA is not set}/acus/state/guard.jsonl"}
  out_name=corpus-windows.jsonl
else
  log=${1:-"${XDG_STATE_HOME:-$HOME/.local/state}/acus/guard.jsonl"}
  out_name=corpus.jsonl
fi
exclude=${GUARD_CORPUS_EXCLUDE:-"$HOME/.config/acus/guard-corpus-exclude.txt"}
redact=${GUARD_CORPUS_REDACT:-"$HOME/.config/acus/guard-corpus-redact.txt"}
if [ ! -f "$exclude" ]; then
  echo "error: no exclusion list at $exclude" >&2
  echo "hint: write the private project names, one per line, to that file or set GUARD_CORPUS_EXCLUDE" >&2
  exit 2
fi
python=${PYTHON:-}
if [ -z "$python" ]; then
  python=python3
  if ! python3 -c pass >/dev/null 2>&1; then
    python=python
  fi
fi
native() {
  if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s\n' "$1"; fi
}
"$python" -I - "$(native "$root")" "$(native "$log")" "$(native "$exclude")" "$(native "$redact")" "$(native "$root/crates/acus-guard/src/tests/$out_name")" "$windows" <<'PY'
import collections, json, os, re, sys

root, log, exclude, redact_file, out, flag = sys.argv[1:7]
windows = flag == "1"
home = os.path.expanduser("~")
names = [n.strip() for n in open(exclude, encoding="utf-8") if n.strip()]
pairs = []
if os.path.exists(redact_file):
    for line in open(redact_file, encoding="utf-8"):
        if "=" in line:
            name, placeholder = (part.strip() for part in line.split("=", 1))
            if name:
                pairs.append((name, placeholder))
pairs.sort(key=lambda pair: len(pair[0]), reverse=True)
redactions = [(re.compile(re.escape(name), re.I), placeholder) for name, placeholder in pairs]
# Never in the public corpus: other projects, the user's name, URLs, secrets, mail addresses,
# release and PR bodies, commit messages, and heredocs whose body is longer than 10 lines.
private = re.compile("|".join(map(re.escape, names + ["luca", "http", "/Documents/projects/"])), re.I)
secret = re.compile(r"token|secret|passw|api.?key|BEGIN .*KEY|@[a-z0-9.-]+\.(com|de|io)", re.I)
release = re.compile(r"\bgh\s+(release|pr)\b|git\s+commit\b.*(\s-[mF]\b|--message|--file)")
heredoc = re.compile(r"<<-?\s*(['\"]?)(\w+)\1")
# Swift commands in the log all come from other projects, so none reach the public corpus.
swift = re.compile(r"swift|xcodebuild|xcrun", re.I)


def long_heredoc(cmd):
    lines = cmd.split("\n")
    for i, line in enumerate(lines):
        for m in heredoc.finditer(line):
            body = []
            for b in lines[i + 1:]:
                if b.strip() == m.group(2):
                    break
                body.append(b)
            if len(body) > 10:
                return True
    return False


def dropped(cmd):
    return bool(private.search(cmd) or secret.search(cmd) or release.search(cmd) or swift.search(cmd) or long_heredoc(cmd))


def normalize(text):
    return text.replace(root, "/repo").replace("$HOME", "~").replace(home, "~")


SEP = r"(?:\\\\|\\|/)"
WIN_CWD = "C:\\repo"
WIN_ROOT = re.compile(r"(?:C:|/c)" + SEP + r"repos" + SEP + r"(?:work|personal)" + SEP + r"[^\\/\s\"'`;|&()]+", re.I)
WIN_HOME = re.compile(r"(?:C:|/c)" + SEP + r"Users" + SEP + r"schmlu(?![\w.-])", re.I)
WIN_CWD_RE = re.compile(r"(?:C:|/c)" + SEP + r"repos" + SEP + r"(?:work|personal)" + SEP + r"[^\\/]+(?P<sub>(?:[\\/]+[^\\/]+)*)[\\/]*$", re.I)


def normalize_win(text):
    text = WIN_ROOT.sub(lambda m: WIN_CWD, text)
    return WIN_HOME.sub(lambda m: "C:\\Users\\user", text)


def win_cwd(cwd):
    m = WIN_CWD_RE.match(cwd or "")
    if not m:
        return WIN_CWD
    parts = [p for p in re.split(r"[\\/]+", m.group("sub")) if p]
    return "\\".join(["C:", "repo", *parts])


def redact(text):
    for rx, placeholder in redactions:
        text = rx.sub(lambda m: placeholder, text)
    return text


seen, logged = set(), []
allowed = {"Bash", "PowerShell"} if windows else {"Bash"}
for line in open(log, encoding="utf-8"):
    try:
        d = json.loads(line)
    except ValueError:
        continue
    call = d.get("call") or ""
    # The log cuts a call at 300 characters; a cut call decides differently from the real one.
    tool = d.get("tool")
    if tool not in allowed or len(call) == 300:
        continue
    if windows:
        cmd, cwd = redact(normalize_win(call)), redact(win_cwd(d.get("cwd")))
    else:
        cmd, cwd = redact(normalize(call)), "/repo"
    if (tool, cmd) in seen:
        continue
    seen.add((tool, cmd))
    if not dropped(cmd):
        logged.append((tool, d.get("kind"), d.get("rule"), cwd, cmd))

CAP = 15
groups = collections.defaultdict(list)
for tool, kind, rule, cwd, cmd in logged:
    dialect = "powershell" if tool == "PowerShell" else "bash"
    groups[(dialect, kind, rule)].append((tool, cwd, cmd))
picked = []
for key in sorted(groups):
    items = groups[key]
    picked += items if len(items) <= CAP else [items[i * len(items) // CAP] for i in range(CAP)]

HAND = [
    "{ echo; cat a.md; } > b",
    "( echo; cat a.md ) > b",
    "{ echo; cat a.md; } | wc -l",
    "if true; then echo; cat a.md; fi > b",
    "for i in 1; do echo; cat a.md; done > c",
    "{ echo; grep x a.rs; } > b; cat a.md",
    "{ echo; head -3 a.md; } > b; tail -n 3 a.md",
    "while read l; do cat a.md; done < list",
    "diff <(cat a.md) <(grep x a.rs)",
    "x=$(cat a.md)",
    "x=$(cat a.md); cat a.md",
    "cat a.md >| b",
    "cat a.md &> b",
    "cat a.md 2>/dev/null",
    "cat a.md && grep x a.rs || echo none",
    "false && cat a.md",
    "grep x a.rs || true",
    "cargo test 2>&1 | tail -n 5; echo ${PIPESTATUS[0]}",
    "print -rl -- ${(f)\"$(cat a.md)\"}",
    "grep -F \"it's\" a.rs",
    "cat my\\ notes.md",
    "grep -n 'fn main' src/*.rs",
    "cat > x.sh <<'EOF'\ngrep foo a.md\nEOF",
    "python3 - <<'PY'\nfrom pathlib import Path\nPath('src/lib.rs').write_text(Path('src/lib.rs').read_text().replace('a', 'b'))\nPY",
    "sed -i 's/a/b/' src/lib.rs",
    "sed -n '1,20p' src/lib.rs",
    "head -n 40 src/lib.rs",
    "command grep x a.rs",
    "command cat a.md",
    "grep -c x a.md $(ls)",
    "git status --short",
    "git diff HEAD~1 -- src/lib.rs",
    "git log --oneline -5",
    "cargo test | tail -n 20",
    "acus ctx 'cargo test' | tail -n 5",
    "cat ~/.claude/settings.json",
    "cat $HOME/.config/x.md",
    "echo hello",
    "ls -la",
    "cargo test",
    "git show HEAD:README.md",
    "command cargo test | tail -n 3",
]


# Cases for the other grammars: the Bash tool under a zsh login shell, and the PowerShell tool.
HAND_ZSH = [
    "print -rl -- ${(f)\"$(cat README.md)\"}",
    "for f in ${(f)\"$(git ls-files src)\"}; do cat $f; done",
    "x=${(f)\"$(cat Cargo.toml)\"}; cat src/main.rs",
    "repeat 3 { cat README.md }",
    "[[ -f src/lib.rs && $x == b* ]] && cat src/lib.rs",
    "cargo test 2>&1 |& tail -5; echo ${pipestatus[1]}",
    "grep -n foo src/*.rs(.)",
    "grep -n foo src/**/*.rs(om[1,3])",
    "diff =(cat a.md) =(cat b.md)",
    "select f in src/*.rs; do cat $f; done",
    "{ cat README.md } always { echo done }",
    "typeset -A m; m=(a 1 b 2); print ${(k)m}; grep -n foo src/lib.rs",
    "echo ${${(f)\"$(cat Cargo.toml)\"}[1]}; head -3 src/main.rs",
    "cat src/lib.rs |& head -3",
    "foreach x (a b); cat $x; end",
    "noglob grep foo src/*.rs",
    "print ${(j:,:)${(f)\"$(ls)\"}} > list.txt; cat src/lib.rs",
]
HAND_PS = [
    "Get-Content src\\lib.rs -Raw",
    "gc src\\lib.rs | Select-String foo",
    "Select-String -Pattern foo -Path src\\*.rs",
    "cd src; Get-Content lib.rs; Get-Date",
    "cd src && type lib.rs",
    "foreach ($f in Get-ChildItem src) { Get-Content $f.FullName }",
    "$x = Get-Content src\\lib.rs; Write-Host $x",
    "Get-ChildItem -Recurse | ForEach-Object { Get-Content $_.FullName } | Out-File all.txt",
    "cargo test 2>&1 | Select-Object -Last 30",
    "cargo build | Select-String error",
    "(Get-Content src\\lib.rs) -replace 'a','b' | Set-Content src\\lib.rs",
    "Get-Content notes.txt | Select-String todo",
    "Get-Content src\\lib.rs > out.txt",
    "Get-ChildItem -Recurse src",
    "Get-Content a.rs,b.rs",
    "if (Test-Path src\\lib.rs) { Get-Content src\\lib.rs }",
    "Write-Host (Get-Content src\\lib.rs)",
    "git log --oneline | Select-String fix",
    "& 'C:\\tools\\rg.exe' foo src",
    "Get-Content src\\lib.rs # acus-skip",
]

HAND_WIN = [
    "cat C:/repo/src/lib.rs",
    "cat C:\\repo\\src\\lib.rs",
    "grep -n fn C:\\repo\\src\\lib.rs",
    "find C:/repo/src -name '*.rs'",
    "head -n 20 /c/repo/src/lib.rs",
    "cat src\\lib.rs",
    "cat /c/Users/user/.claude/settings.json",
    "cat 'C:\\Users\\user\\.claude\\settings.json'",
]
texts = {c[2] for c in picked}
if windows:
    extra = [("Bash", WIN_CWD, cmd) for cmd in HAND_WIN]
    extra += [("PowerShell", WIN_CWD, cmd) for cmd in HAND_PS if cmd not in texts]
else:
    extra = [("Bash", "/repo", cmd) for cmd in HAND]
for tool, cwd, cmd in extra:
    if dropped(cmd) or cmd in texts:
        sys.exit(f"error: hand-written case is dropped or repeated: {cmd!r}")
    picked.append((tool, cwd, cmd))
    texts.add(cmd)

total = len(picked) + (0 if windows else len(HAND_ZSH) + len(HAND_PS))
with open(out, "w", encoding="utf-8") as f:
    for tool, cwd, cmd in picked:
        record = {"tool": tool}
        if windows:
            record["dialect"] = "powershell" if tool == "PowerShell" else "bash"
        record["cwd"] = cwd
        record["cmd"] = cmd
        f.write(json.dumps(record, ensure_ascii=False) + "\n")
    if not windows:
        for cmd in HAND_ZSH:
            f.write(json.dumps({"tool": "Bash", "dialect": "zsh", "cwd": "/repo", "cmd": cmd}, ensure_ascii=False) + "\n")
        for cmd in HAND_PS:
            f.write(json.dumps({"tool": "PowerShell", "dialect": "powershell", "cwd": "/repo", "cmd": cmd}, ensure_ascii=False) + "\n")
print(f"wrote {total} cases ({len(logged)} logged calls passed the filters) to {out}", file=sys.stderr)
PY

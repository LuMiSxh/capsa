#!/usr/bin/env bash
# Rebuilds the golden guard corpus, crates/acus-guard/src/tests/corpus.jsonl, from the hook's log.
#
# Usage: scripts/guard-corpus.sh [GUARD_JSONL]
#   GUARD_JSONL defaults to "$HOME/Library/Application Support/acus/guard.jsonl".
#   GUARD_CORPUS_EXCLUDE names a file of private project names, one per line, kept outside the
#   repository (default "$HOME/.config/acus/guard-corpus-exclude.txt"). The script refuses to run
#   without it, so those names never reach this public repository.
# Each case is {"tool","cwd","cmd"} plus a "dialect" ("zsh", "powershell"; "bash" when
# absent): at most 15 logged calls per (kind, rule), spread evenly over
# the log's order, then the hand-written cases below. After a change, rerun it and review the
# golden snapshot with `cargo insta review`.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
log=${1:-"$HOME/Library/Application Support/acus/guard.jsonl"}
exclude=${GUARD_CORPUS_EXCLUDE:-"$HOME/.config/acus/guard-corpus-exclude.txt"}
if [ ! -f "$exclude" ]; then
  echo "error: no exclusion list at $exclude" >&2
  echo "hint: write the private project names, one per line, to that file or set GUARD_CORPUS_EXCLUDE" >&2
  exit 2
fi
python3 -I - "$root" "$log" "$exclude" "$root/crates/acus-guard/src/tests/corpus.jsonl" <<'PY'
import collections, json, os, re, sys

root, log, exclude, out = sys.argv[1:5]
home = os.path.expanduser("~")
names = [n.strip() for n in open(exclude, encoding="utf-8") if n.strip()]
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


seen, logged = set(), []
for line in open(log, encoding="utf-8"):
    try:
        d = json.loads(line)
    except ValueError:
        continue
    call = d.get("call") or ""
    # The log cuts a call at 300 characters; a cut call decides differently from the real one.
    if d.get("tool") != "Bash" or len(call) == 300:
        continue
    cmd = normalize(call)
    if cmd in seen:
        continue
    seen.add(cmd)
    if not dropped(cmd):
        logged.append((d.get("kind"), d.get("rule"), cmd))

CAP = 15
groups = collections.defaultdict(list)
for kind, rule, cmd in logged:
    groups[(kind, rule)].append(cmd)
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
for cmd in HAND:
    if dropped(cmd) or cmd in picked:
        sys.exit(f"error: hand-written case is dropped or repeated: {cmd!r}")
    picked.append(cmd)

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

with open(out, "w", encoding="utf-8") as f:
    for cmd in picked:
        f.write(json.dumps({"tool": "Bash", "cwd": "/repo", "cmd": cmd}, ensure_ascii=False) + "\n")
    for cmd in HAND_ZSH:
        f.write(json.dumps({"tool": "Bash", "dialect": "zsh", "cwd": "/repo", "cmd": cmd}, ensure_ascii=False) + "\n")
    for cmd in HAND_PS:
        f.write(json.dumps({"tool": "PowerShell", "dialect": "powershell", "cwd": "/repo", "cmd": cmd}, ensure_ascii=False) + "\n")
print(f"wrote {len(picked) + len(HAND_ZSH) + len(HAND_PS)} cases ({len(logged)} logged calls passed the filters) to {out}", file=sys.stderr)
PY

#!/bin/sh
# Acceptance checks for velum against the real srt, one row per spike check in PLAN.md.
# Usage: sh velum/tests/acceptance.sh
# Exit: 0 all rows pass, 1 a row failed, 77 srt is not installed.

if ! command -v srt >/dev/null 2>&1; then
  echo "SKIP: srt not installed"
  exit 77
fi

ROOT=$(cd "$(dirname "$0")/../.." && pwd) || exit 1
cargo build -q -p velum --manifest-path "$ROOT/Cargo.toml" || exit 1
# CARGO_TARGET_DIR, if set, must be absolute: the script changes directory below.
VELUM="${CARGO_TARGET_DIR:-$ROOT/target}/debug/velum"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/velum-acceptance.XXXXXX") || exit 1
LISTENER=
trap '{ [ -z "$LISTENER" ] || { kill "$LISTENER"; wait "$LISTENER"; }; } 2>/dev/null; rm -rf "$WORK"' EXIT
cd "$WORK" || exit 1

pass=0
fail=0

# report NAME STATUS OUTPUT: STATUS 0 means the row passed.
report() {
  if [ "$2" -eq 0 ]; then
    echo "PASS $1"
    pass=$((pass + 1))
  else
    echo "FAIL $1"
    printf '%s\n' "$3" | tail -n 3 | sed 's/^/     | /'
    fail=$((fail + 1))
  fi
}

# denied EXIT OUTPUT: succeeds when the command failed with a sandbox refusal.
denied() {
  [ "$1" -ne 0 ] || return 1
  case "$2" in
    *"Operation not permitted"* | *"Permission denied"*) return 0 ;;
  esac
  return 1
}

out=$("$VELUM" run -- sh -c 'echo ok > inside.txt' 2>&1)
rc=$?
[ "$rc" -eq 0 ] && [ -f inside.txt ]
report "write inside cwd" $? "$out"

out=$("$VELUM" run -- sh -c 'echo x > "$HOME/Documents/velum-test.txt"' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to ~/Documents/velum-test.txt blocked" $? "$out"

out=$("$VELUM" run -- ls "$HOME/.ssh" 2>&1)
rc=$?
denied "$rc" "$out"
report "ls ~/.ssh blocked" $? "$out"

code=$("$VELUM" run -- curl -sS -m 20 -o /dev/null -w '%{http_code}' https://example.com 2>/dev/null)
case "$code" in
  2??) fetched=1 ;;
  *) fetched=0 ;;
esac
report "curl example.com blocked" "$fetched" "HTTP $code"

code=$("$VELUM" run -- curl -sS -m 20 -o /dev/null -w '%{http_code}' https://api.anthropic.com/v1/models 2>/dev/null)
case "$code" in
  000 | 403 | "") reached=0 ;;
  *) reached=1 ;;
esac
[ "$reached" -eq 1 ]
report "curl api.anthropic.com reachable" $? "HTTP $code"

out=$("$VELUM" run -- claude auth status 2>&1)
rc=$?
[ "$rc" -eq 0 ] && printf '%s\n' "$out" | grep -q 'loggedIn.*true'
report "claude auth status loggedIn" $? "$out"

out=$("$VELUM" run -- sh -c 'cargo new --quiet velum-cargo && cd velum-cargo && cargo run --quiet' 2>&1)
rc=$?
[ "$rc" -eq 0 ] && printf '%s\n' "$out" | grep -q 'Hello, world!'
report "cargo new and cargo run" $? "$out"

out=$("$VELUM" run -- sh -c 'mkdir velum-swift && cd velum-swift && swift package init --type executable --name velumswift >/dev/null 2>&1 && swift build' 2>&1)
rc=$?
report "swift package init and swift build via wrapper" "$rc" "$out"

out=$(CAPSA_HUMAN=1 "$VELUM" run -- sh -c 'if [ -n "${CAPSA_HUMAN+x}" ]; then echo visible; exit 1; fi' 2>&1)
rc=$?
report "CAPSA_HUMAN not visible" "$rc" "$out"

out=$(CARGO_REGISTRY_TOKEN=secret CARGO_REGISTRIES_X_TOKEN=secret CARGO_HOME="$HOME/.cargo" "$VELUM" run -- sh -c 'env | grep -E "TOKEN|^CARGO_HOME=" ' 2>&1)
case "$out" in
  *TOKEN*) rc=1 ;;
  *CARGO_HOME=*) rc=0 ;;
  *) rc=1 ;;
esac
report "CARGO_*_TOKEN not visible, CARGO_HOME is" "$rc" "$out"

out=$("$VELUM" run -- sh -c 'echo x > .mcp.json' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to cwd/.mcp.json blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'mkdir .claude' 2>&1)
rc=$?
denied "$rc" "$out"
report "create cwd/.claude blocked" $? "$out"

# Repositories that exist before the run have read-only hooks and config; nested
# project config is read-only everywhere under cwd.
git init -q existing
mkdir -p sub/deep node_modules
git init -q node_modules/dep

out=$("$VELUM" run -- sh -c 'echo x > existing/.git/hooks/pre-commit' 2>&1)
rc=$?
denied "$rc" "$out"
report "hook planted in an existing repo blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'echo x >> existing/.git/config' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to an existing repo's .git/config blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'echo x >> node_modules/dep/.git/config' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to .git/config of a repo under node_modules blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'echo x > sub/.mcp.json' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to nested sub/.mcp.json blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'echo x > sub/deep/.mcp.json' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to nested sub/deep/.mcp.json blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'mkdir sub/.claude' 2>&1)
rc=$?
denied "$rc" "$out"
report "create nested sub/.claude blocked" $? "$out"

# A repository made during the run is writable. srt itself refuses a write to .git/hooks,
# but a git directory built elsewhere and moved into place gets through: the scan after
# the run moves its hooks to the quarantine directory and warns.
out=$("$VELUM" run -- sh -c 'git init -q fresh && git -C fresh status >/dev/null && echo ok > fresh/file' 2>&1)
rc=$?
[ "$rc" -eq 0 ] && [ -d fresh/.git ] && [ -f fresh/file ] && [ -z "$out" ]
report "git init in a new repo works and is silent" $? "$out"

out=$("$VELUM" run -- sh -c 'mkdir -p stage/hooks planted && printf "#!/bin/sh\n" > stage/hooks/pre-commit && printf "[core]\n\thooksPath = /tmp/x\n" > stage/config && mv stage planted/.git' 2>&1)
rc=$?
qdir=$(printf '%s\n' "$out" | sed -n 's/.*moved to \([^:]*\): .*/\1/p' | head -n 1)
[ "$rc" -eq 0 ] && [ ! -e planted/.git/hooks/pre-commit ] && [ -n "$qdir" ] &&
  ls "$qdir" | grep -q pre-commit && printf '%s\n' "$out" | grep -q 'warning: git hooks' &&
  printf '%s\n' "$out" | grep -q 'core.hookspath'
report "hook moved into a new repo is quarantined after the run" $? "$out"
case "$qdir" in
  "$HOME/Library/Application Support/velum/quarantine/"?*) rm -rf "$qdir" ;;
esac

out=$("$VELUM" run -- sh -c 'echo x > planted/.git/hooks/post-commit' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to a hook of a repo made in an earlier run blocked" $? "$out"

out=$(cd "$HOME/Library" && "$VELUM" run -- true 2>&1)
rc=$?
[ "$rc" -ne 0 ] && printf '%s\n' "$out" | grep -q 'refusing to run'
report "run in ~/Library refused" $? "$out"

out=$("$VELUM" run -- sh -c 'echo x > "$HOME/Library/Application Support/velum/run/probe.txt"' 2>&1)
rc=$?
denied "$rc" "$out"
report "write into velum run directory blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'ls "$TMPDIR"' 2>&1)
case "$out" in
  *settings*) rc=1 ;;
  *) rc=0 ;;
esac
report "no settings file in TMPDIR" "$rc" "$out"

out=$(cd / && "$VELUM" run -- true 2>&1)
rc=$?
[ "$rc" -ne 0 ] && printf '%s\n' "$out" | grep -q 'refusing to run'
report "run in / refused" $? "$out"

# Files that an editor or a shell runs on the next start are read-only in cwd and below it.
out=$("$VELUM" run -- sh -c 'mkdir -p .zed && echo "{}" > .zed/tasks.json' 2>&1)
rc=$?
denied "$rc" "$out"
report "create cwd/.zed/tasks.json blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'mkdir -p sub/deep/.vscode && echo "{}" > sub/deep/.vscode/tasks.json' 2>&1)
rc=$?
denied "$rc" "$out"
report "create nested sub/deep/.vscode blocked" $? "$out"

mkdir .vscode
out=$("$VELUM" run -- sh -c 'echo "{}" > .vscode/tasks.json' 2>&1)
rc=$?
denied "$rc" "$out"
report "write into an existing .vscode blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'mkdir sub/.idea' 2>&1)
rc=$?
denied "$rc" "$out"
report "create nested sub/.idea blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'echo x > .zshrc' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to cwd/.zshrc blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'echo x > sub/deep/.zshenv' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to nested sub/deep/.zshenv blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'echo x > sub/.gitconfig' 2>&1)
rc=$?
denied "$rc" "$out"
report "write to nested sub/.gitconfig blocked" $? "$out"

# A bare-repository-shaped directory that exists before the run keeps its hooks and config.
mkdir -p vendor/fixture.git/objects vendor/fixture.git/refs vendor/fixture.git/hooks
echo "ref: refs/heads/main" > vendor/fixture.git/HEAD
printf '[core]\n\tbare = true\n' > vendor/fixture.git/config
out=$("$VELUM" run -- sh -c 'echo x > vendor/fixture.git/hooks/post-checkout' 2>&1)
rc=$?
denied "$rc" "$out" && [ ! -e vendor/fixture.git/hooks/post-checkout ]
report "write to an existing bare repository's hooks blocked" $? "$out"

out=$("$VELUM" run -- sh -c 'echo "[core]" > vendor/fixture.git/config' 2>&1)
rc=$?
denied "$rc" "$out" && grep -q 'bare = true' vendor/fixture.git/config
report "write to an existing bare repository's config blocked" $? "$out"

# A directory that becomes bare-repository-shaped is reported, not changed.
out=$("$VELUM" run -- sh -c 'mkdir -p bare/objects bare/refs && echo "ref: refs/heads/main" > bare/HEAD' 2>&1)
rc=$?
[ "$rc" -eq 0 ] && printf '%s\n' "$out" | grep -q 'bare git repositories' &&
  printf '%s\n' "$out" | grep -q 'safe.bareRepository explicit'
report "bare-repository-shaped directory is reported" $? "$out"

# Network: metadata address and a loopback listener started outside the sandbox.
code=$("$VELUM" run -- curl -sS -m 8 -o /dev/null -w '%{http_code}' http://169.254.169.254/ 2>/dev/null)
case "$code" in
  2?? | 3??) reached=1 ;;
  *) reached=0 ;;
esac
report "curl 169.254.169.254 blocked" "$reached" "HTTP $code"

python3 -c 'import http.server, sys
s = http.server.HTTPServer(("127.0.0.1", 0), http.server.SimpleHTTPRequestHandler)
open(sys.argv[1], "w").write(str(s.server_port))
s.serve_forever()' "$WORK/port" >/dev/null 2>&1 &
LISTENER=$!
tries=0
while [ ! -s "$WORK/port" ] && [ "$tries" -lt 50 ]; do
  sleep 0.1
  tries=$((tries + 1))
done
PORT=$(cat "$WORK/port" 2>/dev/null)
outside=$(curl -sS -m 5 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/" 2>/dev/null)
code=$("$VELUM" run -- curl -sS -m 8 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/" 2>/dev/null)
case "$code" in
  2?? | 3??) reached=1 ;;
  *) reached=0 ;;
esac
[ "$outside" = 200 ] && [ "$reached" -eq 0 ]
report "curl to a loopback listener outside the sandbox blocked" $? "outside HTTP $outside, inside HTTP $code"

out=$("$VELUM" run --dry-run -- true 2>&1)
case "$out" in
  *allowLocalBinding*) rc=1 ;;
  *) rc=0 ;;
esac
report "settings do not enable allowLocalBinding" "$rc" "$out"

echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]

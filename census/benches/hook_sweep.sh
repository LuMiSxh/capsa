#!/bin/sh
# Times `census hook` on a prompt against about 1400 transcript files under `projects/`. The
# session's own transcript gets two rows per call, as a prompt adds rows in a real session.
# `EVENT=SessionStart` times the session start instead. It refreshes the census bulletin from a
# guard log of 1000 lines when the bulletin is older than six hours; `STALE=1` makes it that old
# before each call, so the refresh runs every time.
# Usage: [EVENT=SessionStart] [STALE=1] benches/hook_sweep.sh [census binary]
# Needs hyperfine and python3.
set -eu
BIN=${1:-../target/release/census}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT
export XDG_STATE_HOME=$ROOT/state CLAUDE_CONFIG_DIR=$ROOT/claude HOME=$ROOT/home
export XDG_CONFIG_HOME=$ROOT/config CENSUS_HOOK_DEADLINE_MS=60000
mkdir -p "$ROOT/home"
python3 - "$ROOT" <<'PY'
import json, os, sys, time
root, now = sys.argv[1], int(time.time())
for p in range(20):
    d = f"{root}/claude/projects/app{p}"
    os.makedirs(d)
    for s in range(70):
        n = p * 70 + s
        with open(f"{d}/s{n}.jsonl", "w") as out:
            for i in range(20):
                t = time.strftime("%Y-%m-%dT%H:%M:%SZ",
                                  time.gmtime(now - 6 * 86400 + (s * 20 + i) * 50))
                out.write(json.dumps({"type": "assistant", "timestamp": t, "sessionId": f"s{n}",
                    "message": {"id": f"m{n}-{i}", "model": "claude-opus-5-5",
                    "usage": {"input_tokens": 100 + i, "output_tokens": 50}}}) + "\n")
os.makedirs(f"{root}/state/acus")
with open(f"{root}/state/acus/guard.jsonl", "w") as out:
    for i in range(1000):
        out.write(json.dumps({"ts": now - i, "level": "info", "event": "decision",
           "session": f"s{i % 50}", "cwd": "/tmp", "tool": "Bash", "rule": "grep",
           "kind": ["hint", "rewrite", "deny", "escape"][i % 4],
           "call": f"grep -n x{i} src/a.rs"}) + "\n")
PY
T=$ROOT/claude/projects/app0/s0.jsonl
printf '{"hook_event_name":"%s","session_id":"s0","transcript_path":"%s","cwd":"/tmp"}' \
    "${EVENT:-UserPromptSubmit}" "$T" > "$ROOT/input.json"
"$BIN" hook < "$ROOT/input.json" > /dev/null
cat > "$ROOT/prepare.sh" <<PREP
#!/bin/sh
n=\$(cat "$ROOT/n" 2>/dev/null || echo 0); n=\$((n+1)); echo \$n > "$ROOT/n"
t=\$(date -u +%Y-%m-%dT%H:%M:%SZ)
for k in 1 2; do
printf '{"type":"assistant","timestamp":"%s","sessionId":"s0","message":{"id":"new-%s-%s","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":5}}}\n' "\$t" \$n \$k >> "$T"
done
if [ -n "\${STALE:-}" ]; then printf '%s' '{"guard_follow":{"at":1,"ttl":604800,"value":{}}}' > "$ROOT/state/capsa/bulletin/census.json"; fi
PREP
chmod +x "$ROOT/prepare.sh"
for run in 1 2 3; do
    hyperfine ${HF:-} -N -w 5 -r 50 --prepare "$ROOT/prepare.sh" --input "$ROOT/input.json" \
        --export-json "$ROOT/out$run.json" "$BIN hook"
done
python3 - "$ROOT" <<'PY'
import json
import sys
root = sys.argv[1]
for run in (1, 2, 3):
    r = json.load(open(f"{root}/out{run}.json"))["results"][0]
    t = sorted(r["times"] if "times" in r
               else [m["time_wall_clock"]["value"] for m in r["measurements"]])
    print(f"run {run}: p50 {t[len(t) // 2] * 1000:.1f} ms  p95 {t[int(len(t) * 0.95)] * 1000:.1f} ms")
PY

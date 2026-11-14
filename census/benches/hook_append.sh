#!/bin/sh
# Times `census hook` on a prompt that appends two ledger rows, against a ~2 MB rows file.
# Usage: benches/hook_append.sh [census binary]   (needs hyperfine and python3)
set -eu
BIN=${1:-../target/release/census}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT
export XDG_STATE_HOME=$ROOT/state CLAUDE_CONFIG_DIR=$ROOT/claude HOME=$ROOT/home
export XDG_CONFIG_HOME=$ROOT/config CENSUS_HOOK_DEADLINE_MS=60000
mkdir -p "$ROOT/claude/projects/app" "$ROOT/home"
python3 - "$ROOT" <<'PY'
import json, sys, time
root, now = sys.argv[1], int(time.time())
for f in range(10):
    with open(f"{root}/claude/projects/app/s{f}.jsonl", "w") as out:
        for i in range(1000):
            t = time.strftime("%Y-%m-%dT%H:%M:%SZ",
                              time.gmtime(now - 6 * 86400 + (f * 1000 + i) * 50))
            out.write(json.dumps({"type": "assistant", "timestamp": t, "sessionId": f"s{f}",
                "message": {"id": f"m{f}-{i}", "model": "claude-opus-5-5",
                "usage": {"input_tokens": 100 + i, "cache_read_input_tokens": 5000,
                "cache_creation_input_tokens": 300, "output_tokens": 50}}}) + "\n")
PY
T=$ROOT/claude/projects/app/s0.jsonl
printf '{"hook_event_name":"UserPromptSubmit","session_id":"s0","transcript_path":"%s","cwd":"/tmp"}' \
    "$T" > "$ROOT/input.json"
"$BIN" hook < "$ROOT/input.json" > /dev/null
echo "rows file: $(wc -c < "$(find "$ROOT/state" -name rows.jsonl)") bytes"
cat > "$ROOT/prepare.sh" <<PREP
#!/bin/sh
n=\$(cat "$ROOT/n" 2>/dev/null || echo 0); n=\$((n+1)); echo \$n > "$ROOT/n"
t=\$(date -u +%Y-%m-%dT%H:%M:%SZ)
for k in 1 2; do
printf '{"type":"assistant","timestamp":"%s","sessionId":"s0","message":{"id":"new-%s-%s","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":5}}}\n' "\$t" \$n \$k >> "$T"
done
PREP
chmod +x "$ROOT/prepare.sh"
hyperfine ${HF:-} -N -w 5 -r 50 --prepare "$ROOT/prepare.sh" --input "$ROOT/input.json" \
    --export-json "$ROOT/out.json" "$BIN hook"
python3 - "$ROOT/out.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))["results"][0]
t = sorted(r["times"] if "times" in r
           else [m["time_wall_clock"]["value"] for m in r["measurements"]])
print(f"hook appending rows: p50 {t[len(t) // 2] * 1000:.1f} ms  p95 {t[int(len(t) * 0.95)] * 1000:.1f} ms")
PY

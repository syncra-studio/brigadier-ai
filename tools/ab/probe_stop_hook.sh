#!/bin/bash
# probe_stop_hook.sh <scratch-dir> : PLAN §5's Stop-hook probe (BUILD-PLAN E2), two model calls.
# One `claude -p` with a `--settings` Stop command hook that answers {"decision":"block","reason":…}
# on the first stop and lets the second end. It settles:
#   1. the CLI continues the turn with the reason (the final result holds the word the reason asks for);
#   2. the second stop's event has stop_hook_active true (the first false);
# and it logs each stop event's keys (background_tasks among them, if the CLI sends it).
# The scratch dir must not exist; the caller removes it afterwards (stops.jsonl and result.json are the record).
set -euo pipefail
D=${1:?scratch dir}
case "$D" in /tmp/*|/private/tmp/*) ;; *) echo "probe: the scratch dir must be under /tmp" >&2; exit 2 ;; esac
[ -e "$D" ] && { echo "probe: $D exists" >&2; exit 1; }
mkdir -p "$D/work"
cat > "$D/stop.py" <<'EOF'
import json, os, sys
d = os.path.dirname(os.path.abspath(__file__))
event = json.load(sys.stdin)
with open(os.path.join(d, "stops.jsonl"), "a") as f:
    f.write(json.dumps(event) + "\n")
n = sum(1 for _ in open(os.path.join(d, "stops.jsonl")))
if n == 1:
    print(json.dumps({"decision": "block",
                      "reason": "Before you stop: reply with the single word PINEAPPLE on a line of its own."}))
EOF
python3 - "$D" <<'EOF'
import json, shlex, sys
d = sys.argv[1]
json.dump({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "python3 " + shlex.quote(d + "/stop.py"),
                                          "timeout": 30}]}]}}, open(d + "/settings.json", "w"))
EOF
cd "$D/work"
claude -p "Reply with the single word ready." --model sonnet --settings "$D/settings.json" \
  --output-format json --no-session-persistence > "$D/result.json" 2> "$D/stderr.txt" || true
python3 - "$D" <<'EOF'
import json, sys
d = sys.argv[1]
stops = [json.loads(l) for l in open(d + "/stops.jsonl")] if __import__("os").path.exists(d + "/stops.jsonl") else []
try:
    result = json.load(open(d + "/result.json"))
except Exception as err:
    result = {"unreadable": repr(err), "raw": open(d + "/result.json").read()[-2000:],
              "stderr": open(d + "/stderr.txt").read()[-2000:]}
out = {
    "stops": len(stops),
    "stop_hook_active": [s.get("stop_hook_active") for s in stops],
    "stop_event_keys": [sorted(s) for s in stops],
    "result_text": result.get("result"),
    "continued_with_reason": "PINEAPPLE" in (result.get("result") or ""),
    "num_turns": result.get("num_turns"),
    "is_error": result.get("is_error"),
    "usage": result.get("usage"),
    "model_usage": result.get("modelUsage"),
}
print(json.dumps(out, indent=1))
ok = out["continued_with_reason"] and out["stop_hook_active"][:2] == [False, True] and out["stops"] == 2
print("PROBE", "OK" if ok else "UNEXPECTED")
EOF

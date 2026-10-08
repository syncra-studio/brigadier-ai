#!/bin/bash
# dlg_start.sh <arm-dir> <task.md> : the /delegator arm. Opens a new cmux workspace "AB <arm>" in
# the arm's clone running the user's own unmodified `claude` (Opus 5.5, effort high, permissions
# bypassed like the user's Delegator tab), with no DLG_* variables from the caller so it starts a
# run of its own. Then types "/delegator <the task's request verbatim> (Image #n is the file
# <path>)" and Enter; that Enter is t0.
# Flags checked against `claude --help` (2.1.292: --dangerously-skip-permissions, --model, --effort)
# and `cmux new-workspace --help` (0.65.0: --name, --cwd, --command, --focus).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
A=$(cd "${1:?arm dir}" && pwd); taskf=${2:?task file}
name="AB $(basename "$A")"
[ -e "$A/surface" ] && { echo "dlg_start: $A already has a tab" >&2; exit 1; }
cmux tree --all | grep -qF "\"$name\"" && { echo "workspace $name already exists" >&2; exit 1; }
created=$(CMUX_QUIET=1 cmux workspace create --name "$name" --cwd "$A/repo" \
  --command "env -u DLG_RUN -u DLG_WORKER -u DLG_WORKER_DIR claude --dangerously-skip-permissions --model claude-opus-5-5 --effort high" \
  --focus false)
ws=$(grep -oE 'workspace:[0-9]+' <<<"$created" | head -1)
[ -n "$ws" ] || { echo "dlg_start: no workspace ref in: $created" >&2; exit 1; }
sleep 2
sid=$(cmux --id-format both tree --workspace "$ws" | grep ' surface surface:' | grep -oE '[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}' | head -1)
[ -n "$sid" ] || { echo "dlg_start: no surface in $ws" >&2; exit 1; }
echo "$sid" > "$A/surface"; echo "$ws" > "$A/workspace"
echo "workspace $ws surface $sid"
for _ in $(seq 1 90); do
  scr=$("$HERE/cmx.sh" "$A" read-screen --lines 25 2>/dev/null || true)
  # The folder-trust and bypass-permissions prompts: choose their "Yes" option.
  if grep -qE "Yes, I trust this folder|Yes, I accept" <<<"$scr"; then
    grep -qE "❯ *[0-9.]* *Yes" <<<"$scr" || { "$HERE/cmx.sh" "$A" send-key down; sleep 1; }
    "$HERE/cmx.sh" "$A" send-key enter; sleep 3; continue
  fi
  grep -q "bypass permissions on" <<<"$scr" && break
  sleep 1
done
text=$(python3 - "$HERE" "$taskf" <<'PY'
import sys
sys.path.insert(0, sys.argv[1])
from task import load_task, SAFETY
t = load_task(sys.argv[2])
text = t["request"]
if t.get("image"):
    text += f" (Image #{t['image-number']} is the file {t['image']})"
print(text + " " + SAFETY)
PY
)
"$HERE/cmx.sh" "$A" send -- "/delegator $text"
sleep 1
t0=$(python3 -c 'import time;print(int(time.time()*1000))')
"$HERE/cmx.sh" "$A" send-key enter
python3 - "$A" "$t0" "$sid" "$ws" "/delegator $text" <<'PY'
import json, sys
a, t0, sid, ws, text = sys.argv[1:6]
json.dump({"arm": "delegator", "t0_ms": int(t0), "surface": sid, "workspace": ws, "text": text}, open(a + "/start.json", "w"), indent=1)
PY
cat "$A/start.json"

#!/bin/bash
# dlg_say.sh <arm-dir> <question|approval|merge|push> : answers the /delegator arm's coordinator by
# the A/B response policy (POLICY.md), in its own tab only, and logs the intervention with its time
# in <arm-dir>/interventions.jsonl.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
A=${1:?arm dir}; kind=${2:?kind}
case "$kind" in
  question) text="Go with your recommendation." ;;
  approval) text="Go ahead." ;;
  merge) text="Leave it on its branch; don't merge." ;;
  push) text="No push: leave the work on its branch." ;;
  *) echo "dlg_say: unknown kind $kind" >&2; exit 2 ;;
esac
"$HERE/cmx.sh" "$A" send -- "$text"
sleep 1
ms=$(python3 -c 'import time;print(int(time.time()*1000))')
"$HERE/cmx.sh" "$A" send-key enter
python3 -c 'import json,sys; print(json.dumps({"at_ms": int(sys.argv[1]), "kind": sys.argv[2], "text": sys.argv[3]}))' "$ms" "$kind" "$text" >> "$A/interventions.jsonl"
echo "sent ($kind) at $ms: $text"

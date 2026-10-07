#!/bin/bash
# reqdone.sh <arm-dir> : returns once the arm's request is no longer "working".
A=${1:?arm dir}
until python3 - "$A" <<'PY'
import json, sys
st = None
for l in open(sys.argv[1] + "/rec/events.jsonl"):
    e = json.loads(l)["event"]
    if e["type"] == "requestUpdated": st = e["request"]["state"]["type"]
sys.exit(0 if st not in (None, "working") else 1)
PY
do sleep 5; done
echo "request ended"

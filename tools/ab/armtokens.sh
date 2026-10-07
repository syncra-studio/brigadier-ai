#!/bin/bash
# armtokens.sh <arm-dir> <end_ms> : a Brigadier arm's tokens three ways, which must agree:
# routing.sqlite turn_usage (the number reported), the session transcripts per the daemon's own
# manifest (Codex child threads included), and the daemon's recorded worker usage events.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); A=${1:?arm dir}; END=${2:?end ms}
T0=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['t0_ms'])" "$A/start.json")
python3 "$HERE/brig_tokens.py" "$A" "$END" --json "$A/tokens.json"
python3 "$HERE/brig_manifest.py" "$A" > "$A/manifest.json"
echo "--- transcripts (KEEP=$A/transcripts):"
KEEP="$A/transcripts" python3 "$HERE/tokens.py" "$A/manifest.json" "$T0" "$END" --json "$A/tokens-transcripts.json"
echo "--- the daemon's worker usage events:"
python3 "$HERE/evtokens.py" "$A" "$END"
echo "--- turn_usage against the transcripts:"
# The arm's number is turn_usage plus the Codex child threads the daemon didn't meter. A range
# review's child is in its review's row already; a daemon from phase 2 on meters a worker's or
# thread's auto-review threads itself (listed in metered_child_threads). Only the others are
# added from their rollouts, so none counts twice.
python3 - "$A/tokens.json" "$A/tokens-transcripts.json" <<'PY'
import json, sys
brig = json.load(open(sys.argv[1])); seen = json.load(open(sys.argv[2]))
metered = brig["total"]["raw"]; known = set(brig.get("metered_child_threads", []))
extra = [r for r in seen["rows"] if r["session"].get("parent") and r["session"].get("role") != "review child"
         and r["session"]["id"] not in known and r["usage"]]
added = sum(r["usage"]["raw"] for r in extra)
for r in extra:
    print(f"  not metered by the daemon: {r['session']['label']} {r['session']['id']} {r['usage']['raw']:,} raw")
total = metered + added; transcripts = seen["total"]["total_raw"]
print(f"turn_usage {metered:,} raw ({len(known)} child threads metered by the daemon) + {added:,} raw from "
      f"{len(extra)} child threads it didn't = {total:,} raw; transcripts {transcripts:,} raw, difference {transcripts - total:+,}")
PY

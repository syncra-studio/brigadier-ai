#!/bin/bash
# armtokens.sh <arm-dir> <end_ms> : a Brigadier arm's tokens three ways, which must agree:
# routing.sqlite turn_usage (the number reported), the session transcripts per the daemon's own
# manifest, and the daemon's recorded worker usage events.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); A=${1:?arm dir}; END=${2:?end ms}
T0=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['t0_ms'])" "$A/start.json")
python3 "$HERE/brig_tokens.py" "$A" "$END" --json "$A/tokens.json"
python3 "$HERE/brig_manifest.py" "$A" > "$A/manifest.json"
echo "--- transcripts (KEEP=$A/transcripts):"
KEEP="$A/transcripts" python3 "$HERE/tokens.py" "$A/manifest.json" "$T0" "$END" --json "$A/tokens-transcripts.json"
echo "--- the daemon's worker usage events:"
python3 "$HERE/evtokens.py" "$A" "$END"

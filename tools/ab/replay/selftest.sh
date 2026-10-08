#!/usr/bin/env bash
# Replays fixture/ (the first 10 s of a phase-3 T1 arm: the thread's first tool calls) through
# APP's row code and checks that the folded row of the thread's finished tool steps counts:
# the block stops being "only Thinking" when its first tool steps arrive (4.8 s), not when the
# first worker row shows. Usage: [APP=<apps/desktop>] tools/ab/replay/selftest.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
arm="$(mktemp -d)"
trap 'rm -rf "$arm"' EXIT
cp -R "$here/fixture/." "$arm/"
ARM="$arm" node "$here/run.mjs" >/dev/null
python3 - "$arm/ui-replay.json" <<'PY'
import json, sys
replay = json.load(open(sys.argv[1]))
summary = replay["summary"]
frame = next(f for f in replay["frames"] if f["t"] == 6)
assert any("Searched project memory" in row for row in frame["actionRows"]), frame
assert summary["firstActionRowS"] <= 5, summary
assert summary["longestOnlyThinking"]["length"] <= 5, summary
print("replay selftest passed:", frame["actionRows"][0], "| longest only Thinking",
      summary["longestOnlyThinking"])
PY

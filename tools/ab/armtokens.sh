#!/bin/bash
# armtokens.sh <arm-dir> <end_ms> [--also DIR ...] : a Brigadier arm's tokens three ways, which must
# agree: routing.sqlite turn_usage (the number reported), the session transcripts per the daemon's
# own manifest (Codex descendants and any --also dev data dir included), and the daemon's recorded
# worker usage events. The reconciled per-provider totals (turn_usage plus the threads the daemon
# didn't meter) are written into <arm>/tokens.json under "reconciled". It fails loudly (exit 3,
# "complete": false) on unknown usage or a difference between turn_usage and the transcripts.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); A=${1:?arm dir}; END=${2:?end ms}; shift 2
T0=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['t0_ms'])" "$A/start.json")
python3 "$HERE/brig_tokens.py" "$A" "$END" --json "$A/tokens.json" >/dev/null
python3 "$HERE/brig_manifest.py" "$A" "$@" > "$A/manifest.json"
echo "--- transcripts (KEEP=$A/transcripts):"
rc=0
KEEP="$A/transcripts" python3 "$HERE/tokens.py" "$A/manifest.json" "$T0" "$END" --json "$A/tokens-transcripts.json" || rc=$?
echo "--- the daemon's worker usage events:"
python3 "$HERE/evtokens.py" "$A" "$END"
echo "--- turn_usage against the transcripts:"
# The arm's number is turn_usage plus the Codex child threads the daemon didn't meter. A range
# review's child is in its review's row already; a daemon from phase 2 on meters a worker's or
# thread's auto-review threads itself (listed in metered_child_threads). Only the others are
# added from their rollouts, so none counts twice. Dev-app sessions (--also) are added too.
python3 - "$A/tokens.json" "$A/tokens-transcripts.json" "$rc" <<'PY'
import json, sys
path = sys.argv[1]; brig = json.load(open(path)); seen = json.load(open(sys.argv[2])); rc = int(sys.argv[3])
known = set(brig.get("metered_child_threads", []))
extra = [r for r in seen["rows"] if r["usage"] and r["session"]["id"] not in known and (
    r["session"]["role"] == "worker-dev-app" or (r["session"].get("parent") and r["session"].get("role") != "review child"))]
prov = {k: dict(v) for k, v in brig["by_provider"].items()}
for r in extra:
    u = r["usage"]; p = r["session"]["provider"]
    row = prov.setdefault(p, {"input_uncached": 0, "cache_read": 0, "cache_write": "not reported" if p == "codex" else 0,
                              "output": 0, "raw": 0, "raw_without_cache_reads": 0, "turns": 0})
    if p == "codex":
        add = {"input_uncached": u["input_tokens"] - u["cached_input_tokens"], "cache_read": u["cached_input_tokens"], "output": u["output_tokens"]}
    else:
        add = {"input_uncached": u["input"], "cache_read": u["cache_read"], "output": u["output"]}
        row["cache_write"] += u["cache_write"]
    for k, v in add.items(): row[k] += v
    row["raw"] += u["raw"]; row["raw_without_cache_reads"] += u["raw"] - add["cache_read"]
    print(f"  not metered by the daemon: {r['session']['label']} {r['session']['id']} {u['raw']:,} raw")
total = sum(v["raw"] for v in prov.values()); transcripts = seen["total"]["total_raw"]
diff = transcripts - total
complete = rc == 0 and seen.get("complete", True) and diff == 0
brig["reconciled"] = {"by_provider": prov, "total": {"raw": total, "raw_without_cache_reads": sum(v["raw_without_cache_reads"] for v in prov.values())},
                      "added_threads": [r["session"]["id"] for r in extra], "transcripts_raw": transcripts, "difference": diff,
                      "transcripts_complete": seen.get("complete", True), "complete": complete}
json.dump(brig, open(path, "w"), indent=1)
print(f"turn_usage {brig['total']['raw']:,} raw ({len(known)} child threads metered by the daemon) + {len(extra)} threads it didn't "
      f"= {total:,} raw; transcripts {transcripts:,} raw, difference {diff:+,}")
for p, v in prov.items(): print(f"  {p}: {v}")
if not complete:
    print("armtokens: INCOMPLETE evidence (unknown usage or an unexplained difference); see above", file=sys.stderr); sys.exit(3)
print("complete")
PY

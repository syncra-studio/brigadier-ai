#!/usr/bin/env python3
"""A V3 attempt's usage through settlement, the way tools/ab/armtokens.sh counts an arm, for one
conversation of a daemon that also ran a warm-up session (and, in the template, Brain jobs):
- `brig_tokens.py`: the conversation's routing.sqlite `turn_usage` rows in [t0, settled] (the
  number reported), with project-level background work (Brain jobs) listed apart;
- `brig_manifest.py`, then only the sessions this conversation owns (its thread, its tasks, its
  reviews and their Codex descendants) go to `tokens.py` over the hard-linked transcripts; every
  other session (the warm-up's thread, the template's Brain jobs) is listed in manifest-all.json
  and, if it used anything inside the window, reported as `other_in_window`;
- the same reconciliation as armtokens.sh: turn_usage plus the Codex child threads the daemon
  didn't meter must equal the transcripts, or the result is marked incomplete (exit 3).
usage: v3tokens.py <run-dir> <settled_ms>   writes tokens.json, manifest.json, manifest-all.json,
       tokens-transcripts.json"""
import json, os, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__)); AB = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import v3lib  # noqa: E402

run, end = sys.argv[1], int(sys.argv[2])
rec = v3lib.Record(run)
subprocess.run([sys.executable, os.path.join(AB, "brig_tokens.py"), run, str(end), "--json", os.path.join(run, "tokens.json")],
               check=True, stdout=subprocess.DEVNULL)
allm = json.loads(subprocess.run([sys.executable, os.path.join(AB, "brig_manifest.py"), run], check=True,
                                 capture_output=True, text=True).stdout)
json.dump(allm, open(os.path.join(run, "manifest-all.json"), "w"), indent=1)
owners = {f"orch:{rec.conv}"} | {f"task:{t}" for t in rec.tasks} | {f"review:{r}" for r in rec.reviews}
mine = [s for s in allm if s["owner"] in owners]
json.dump(mine, open(os.path.join(run, "manifest.json"), "w"), indent=1)
others = [s for s in allm if s["owner"] not in owners]
env = dict(os.environ, KEEP=os.path.join(run, "transcripts"))
p = subprocess.run([sys.executable, os.path.join(AB, "tokens.py"), os.path.join(run, "manifest.json"), str(rec.t0), str(end),
                    "--json", os.path.join(run, "tokens-transcripts.json")], env=env, capture_output=True, text=True)
print(p.stdout[-3000:], p.stderr[-1000:])
rc = p.returncode
other_in_window = []
if others:
    tmp = os.path.join(run, "manifest-others.json")
    json.dump(others, open(tmp, "w"))
    q = subprocess.run([sys.executable, os.path.join(AB, "tokens.py"), tmp, str(rec.t0), str(end), "--json", tmp + ".tokens"],
                       env=env, capture_output=True, text=True)
    if os.path.exists(tmp + ".tokens"):
        for r in json.load(open(tmp + ".tokens"))["rows"]:
            if r["usage"] and r["usage"].get("raw"):
                other_in_window.append({"label": r["session"]["label"], "owner": r["session"]["owner"], "raw": r["usage"]["raw"]})
        os.remove(tmp + ".tokens")
    os.remove(tmp)

# armtokens.sh's reconciliation, unchanged in substance.
path = os.path.join(run, "tokens.json"); brig = json.load(open(path))
seen = json.load(open(os.path.join(run, "tokens-transcripts.json")))
known = set(brig.get("metered_child_threads", []))
extra = [r for r in seen["rows"] if r["usage"] and r["session"]["id"] not in known and (
    r["session"]["role"] == "worker-dev-app" or (r["session"].get("parent") and r["session"].get("role") != "review child"))]
prov = {k: dict(v) for k, v in brig["by_provider"].items()}
for r in extra:
    u = r["usage"]; pv = r["session"]["provider"]
    row = prov.setdefault(pv, {"input_uncached": 0, "cache_read": 0, "cache_write": "not reported" if pv == "codex" else 0,
                               "output": 0, "raw": 0, "raw_without_cache_reads": 0, "turns": 0})
    if pv == "codex":
        add = {"input_uncached": u["input_tokens"] - u["cached_input_tokens"], "cache_read": u["cached_input_tokens"], "output": u["output_tokens"]}
    else:
        add = {"input_uncached": u["input"], "cache_read": u["cache_read"], "output": u["output"]}
        row["cache_write"] += u["cache_write"]
    for k, v in add.items():
        row[k] += v
    row["raw"] += u["raw"]; row["raw_without_cache_reads"] += u["raw"] - add["cache_read"]
total = sum(v["raw"] for v in prov.values()); transcripts = seen["total"]["total_raw"]
diff = transcripts - total
complete = rc == 0 and seen.get("complete", True) and diff == 0
brig["reconciled"] = {"by_provider": prov, "total": {"raw": total, "raw_without_cache_reads": sum(v["raw_without_cache_reads"] for v in prov.values()),
                                                      "cache_read": sum(v["cache_read"] for v in prov.values())},
                      "added_threads": [r["session"]["id"] for r in extra], "transcripts_raw": transcripts, "difference": diff,
                      "transcripts_complete": seen.get("complete", True), "complete": complete}
brig["other_in_window"] = other_in_window
json.dump(brig, open(path, "w"), indent=1)
print(f"turn_usage {brig['total'].get('raw', 0):,} raw + {len(extra)} unmetered threads = {total:,}; transcripts {transcripts:,}; "
      f"difference {diff:+,}; other sessions using tokens in the window: {len(other_in_window)}")
print("complete" if complete else "INCOMPLETE")
sys.exit(0 if complete else 3)

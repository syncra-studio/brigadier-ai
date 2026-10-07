#!/usr/bin/env python3
"""Terse timeline of a recorded arm. usage: timeline.py <arm-dir> [--all]"""
import json, sys, collections
arm = sys.argv[1]; full = "--all" in sys.argv
st = json.load(open(arm + "/start.json")); t0 = st["t0_ms"]; conv = st["conversation"]
evs = [json.loads(l) for l in open(arm + "/rec/events.jsonl")]
cnt = collections.Counter()
def ts(ms): s = (ms - t0) / 1000; return f"{int(s//60):3d}:{s%60:04.1f}"
last_task = {}
for e in evs:
    ev = e["event"]; t = ev.get("type"); cnt[t] += 1
    if e["atMs"] < t0 - 1000: continue
    line = None
    if t == "messageAppended":
        m = ev["message"]; line = f"msg {m['role']}: {m.get('text','')[:160]!r}"
    elif t == "taskUpdated":
        k = ev["task"]; key = (k["id"]); s = json.dumps(k.get("state"))[:60]
        if last_task.get(key) != s:
            last_task[key] = s; line = f"task {k.get('title','')[:50]!r} {k.get('kind')} {s} model={json.dumps(k.get('route', {}).get('choice') if isinstance(k.get('route'), dict) else None)}"
    elif t in ("approvalUpdated", "questionUpdated", "planUpdated"):
        line = f"CARD {t} {json.dumps(ev)[:200]}"
    elif t in ("runStateChanged", "requestUpdated", "conversationWaiting", "conversationNotice", "decidedForYou", "waitingOnYou", "machineStepped"):
        line = f"{t} {json.dumps(ev)[:200]}"
    elif t == "orchestratorStepped" and full:
        line = f"orch {json.dumps(ev)[:200]}"
    elif t == "workerStepped" and full:
        line = f"worker {json.dumps(ev)[:200]}"
    elif t == "rawSessionCreated":
        s = ev["session"]; line = f"raw+ {json.dumps(s)[:220]}"
    if line: print(ts(e["atMs"]), line)
print(dict(cnt))

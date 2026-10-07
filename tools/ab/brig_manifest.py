#!/usr/bin/env python3
"""Brigadier session manifest from the arm's own event log (cleanupRecorded sessions/threads,
with their owners). usage: brig_manifest.py <arm-dir> > manifest.json"""
import json, sys
arm = sys.argv[1]; conv = json.load(open(arm + "/start.json"))["conversation"]
tasks = {}; brain = set(); out = []; seen = set()
for l in open(arm + "/rec/events.jsonl"):
    e = json.loads(l)["event"]
    if e["type"] == "taskUpdated":
        t = e["task"]; tasks[t["id"]] = t
for l in open(arm + "/rec/events.jsonl"):
    env = json.loads(l); e = env["event"]
    if e["type"] != "cleanupRecorded": continue
    a = e["artifact"]; owner = e["owner"]
    if a["type"] not in ("claudeSession", "codexThread"): continue
    sid = a.get("sessionId") or a.get("threadId")
    if sid in seen: continue
    seen.add(sid)
    kind, _, oid = owner.partition(":")
    if kind == "task":
        t = tasks.get(oid, {}); role = t.get("kind", "task"); label = f"task {t.get('title','?')}"
    elif kind == "orch": role = "orchestrator"; label = f"orchestrator {oid[:8]}"
    elif kind == "brain": role = "brain(bg)"; label = f"brain job {oid[:8]}"
    else: role = kind; label = owner
    out.append({"label": label, "role": role, "provider": "claude" if a["type"] == "claudeSession" else "codex", "id": sid, "owner": owner, "at_ms": env["atMs"]})
print(json.dumps(out, indent=1))

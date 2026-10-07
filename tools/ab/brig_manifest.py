#!/usr/bin/env python3
"""Brigadier session manifest from the arm's own event log (cleanupRecorded sessions/threads,
with their owners), plus every Codex child thread of a listed thread: a range review's and an
Approve-for-me auto-review's ("guardian") run in their own thread, which the event log doesn't
list and whose use the parent thread doesn't report. Their rollouts name the parent in
`session_meta.parent_thread_id`. usage: brig_manifest.py <arm-dir> > manifest.json"""
import glob, json, os, sys
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
rollouts = glob.glob(f"{arm}/transcripts/codex/rollout-*.jsonl") + glob.glob(os.path.expanduser("~/.codex/sessions/*/*/*/rollout-*.jsonl"))
metas = []
for f in rollouts:
    try: metas.append(json.loads(open(f).readline())["payload"])
    except Exception: continue
# A daemon older than 507ad4ef doesn't record a Codex review's thread: it is the thread that ran
# in the review's checkout, `review-<the id's last 12>`.
reviews = {}
for l in open(arm + "/rec/events.jsonl"):
    env = json.loads(l); e = env["event"]
    if e["type"] == "reviewUpdated" and e["review"]["conversationId"] == conv and e["review"]["reviewer"] == "codex":
        reviews.setdefault(e["review"]["id"], (e["review"], env["atMs"]))
owners = {s["owner"] for s in out}
for rid, (r, at) in reviews.items():
    if f"review:{rid}" in owners: continue
    for meta in metas:
        if meta.get("parent_thread_id") is None and meta.get("cwd", "").endswith(f"/review-{rid[-12:]}") and meta.get("id") not in seen:
            seen.add(meta["id"])
            out.append({"label": f"review {r['kind']} {rid[:8]}", "role": "review", "provider": "codex", "id": meta["id"], "owner": f"review:{rid}", "at_ms": at})
parents = {s["id"]: s for s in out if s["provider"] == "codex"}
for meta in metas:
    parent = parents.get(meta.get("parent_thread_id"))
    if not parent or meta.get("id") in seen: continue
    seen.add(meta["id"])
    kind = meta.get("thread_source") or "child"
    out.append({"label": f"{parent['label']} · {kind}", "role": f"{parent['role']} child", "provider": "codex", "id": meta["id"], "owner": parent["owner"], "at_ms": parent["at_ms"], "parent": parent["id"]})
print(json.dumps(out, indent=1))

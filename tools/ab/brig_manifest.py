#!/usr/bin/env python3
"""Brigadier session manifest from the arm's own event log (cleanupRecorded sessions/threads,
with their owners), plus every Codex child thread of a listed thread: a range review's and an
Approve-for-me auto-review's ("guardian") run in their own thread, which the event log doesn't
list and whose use the parent thread doesn't report. Their rollouts name the parent in
`session_meta.parent_thread_id`; descendants are followed recursively.
usage: brig_manifest.py <arm-dir> [--also DIR ...] > manifest.json"""
import glob, json, os, re, sys
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
# --also DIR: a dev Brigadier data dir (or other folder) a worker ran model turns in: every Claude
# session under it (by project slug) and every Codex session started in it after t0 counts too.
t0 = json.load(open(arm + "/start.json"))["t0_ms"]
H = os.path.expanduser("~")
also = [sys.argv[i + 1] for i, v in enumerate(sys.argv) if v == "--also"]
for d in also:
    d = os.path.realpath(d)
    for c in {d, d[len("/private"):] if d.startswith("/private/") else d}:
        for f in sorted(glob.glob(f"{H}/.claude/projects/{re.sub(r'[^A-Za-z0-9]', '-', c)}*/*.jsonl")):
            sid = os.path.basename(f)[:-6]
            if os.path.getmtime(f) * 1000 >= t0 and sid not in seen:
                seen.add(sid)
                out.append({"label": f"dev app {os.path.basename(os.path.dirname(f))[:60]}", "role": "worker-dev-app", "provider": "claude", "id": sid, "owner": f"also:{d}", "at_ms": t0})
        for meta in metas:
            cwd = meta.get("cwd") or ""
            if (cwd == c or cwd.startswith(c + "/")) and meta.get("id") not in seen and meta.get("parent_thread_id") is None:
                seen.add(meta["id"])
                out.append({"label": f"dev app codex {meta['id'][:13]}", "role": "worker-dev-app", "provider": "codex", "id": meta["id"], "owner": f"also:{d}", "at_ms": t0})
# Codex descendants, recursively: a review's, an auto-review's or a spawned agent's thread names
# its parent in `session_meta.parent_thread_id`.
grew = True
while grew:
    grew = False
    parents = {s["id"]: s for s in out if s["provider"] == "codex"}
    for meta in metas:
        parent = parents.get(meta.get("parent_thread_id"))
        if not parent or meta.get("id") in seen: continue
        seen.add(meta["id"]); grew = True
        kind = meta.get("thread_source") or "child"
        role = parent["role"] if parent["role"].endswith(" child") else f"{parent['role']} child"
        out.append({"label": f"{parent['label']} · {kind}", "role": role, "provider": "codex", "id": meta["id"], "owner": parent["owner"], "at_ms": parent["at_ms"], "parent": parent["id"]})
print(json.dumps(out, indent=1))

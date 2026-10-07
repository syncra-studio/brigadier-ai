#!/usr/bin/env python3
"""An overnight smoke over IPC: a new session on the arm's project, a proposed run of the given
small phases, the user's Start, then waits until the run is finished. Run the recorder alongside
(it answers cards like the user and logs the run's events).
usage: overnight.py <data-dir> <repo> <plan.json> <recorder events.jsonl> [--timeout-min 90]
plan.json: {"name", "goal", "phases": [{"name", "scope", "doneWhen": [..]}]}"""
import argparse, json, os, sys, time, uuid
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bipc import req

ap = argparse.ArgumentParser()
ap.add_argument("data"); ap.add_argument("repo"); ap.add_argument("plan"); ap.add_argument("events")
ap.add_argument("--timeout-min", type=float, default=90)
a = ap.parse_args()
plan = json.load(open(a.plan))
repo = os.path.realpath(a.repo)
cat = req(a.data, {"method": "getCatalog"})["value"]["catalog"]
proj = [p for p in cat["projects"] if p["repos"] and os.path.realpath(p["repos"][0]["path"]) == repo][0]
setup = {"type": "session", "repo": proj["repos"][0]["path"],
         "environment": {"type": "newWorktree", "base": "main", "branch": None},
         "permission": cat["settings"]["defaultPermission"],
         "orchestrator": {"provider": "claude", "model": "opus", "effort": "high"}, "planMode": False}
conv = req(a.data, {"method": "createConversation", "kind": "session", "projectId": proj["id"],
                    "title": None, "setup": setup})["value"]["conversation"]["id"]
proposed = {"name": plan["name"], "goal": plan.get("goal"), "rules": plan.get("rules"), "sources": [],
            "phases": [{"number": i + 1, "name": p["name"], "scope": p["scope"],
                        "doneWhen": p.get("doneWhen", []), "dependsOn": []}
                       for i, p in enumerate(plan["phases"])]}
r = req(a.data, {"method": "proposeOvernight", "conversationId": conv, "commandId": str(uuid.uuid4()),
                 "words": plan.get("words", plan["name"]), "plan": proposed})
if r.get("status") != "ok":
    sys.exit(f"propose: {json.dumps(r)[:800]}")
run = r["value"]["run"]
print("proposed", conv, run["id"], "revision", run["revision"], "state", run["state"], flush=True)
r = req(a.data, {"method": "startOvernight", "conversationId": conv, "runId": run["id"],
                 "commandId": str(uuid.uuid4()), "revision": run["revision"]})
if r.get("status") != "ok":
    sys.exit(f"start: {json.dumps(r)[:800]}")
t0 = time.time()
print("started", time.strftime("%T"), int(t0 * 1000), flush=True)
json.dump({"conversation": conv, "run": run["id"], "t0_ms": int(t0 * 1000)},
          open(os.path.join(os.path.dirname(os.path.abspath(a.plan)), "overnight-start.json"), "w"))
last = None
while time.time() - t0 < a.timeout_min * 60:
    now = None
    for l in open(a.events):
        e = json.loads(l)["event"]
        if e["type"] == "overnightUpdated" and e["run"]["id"] == run["id"]:
            now = e["run"]
    if now:
        line = (now["state"], now.get("stop"), now.get("verifiedCommit"))
        if line != last:
            print(time.strftime("%T"), json.dumps(line), flush=True); last = line
        if now["state"] == "finished":
            print("finished", int(time.time() * 1000), "accepted tip", now.get("verifiedCommit"))
            sys.exit(0)
    time.sleep(10)
sys.exit("timed out")

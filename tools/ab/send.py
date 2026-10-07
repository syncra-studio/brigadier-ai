#!/usr/bin/env python3
"""Creates a session on the arm's project and sends a frozen task's request, with its image
inline the way the composer sends a pasted one (`[Image #n]` in the text, the attachment marked
inline n). The clock starts when sendMessage is sent.
usage: send.py <data-dir> <repo> <task.md> <arm-dir> [--environment newWorktree|localCheckout]
               [--permission LEVEL] [--model opus] [--effort high]
The task file's `request:` block is sent verbatim; `image:` names the file and `image-number:` its
marker. Permission defaults to the daemon's default for new sessions."""
import argparse, base64, json, mimetypes, os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bipc import req
from task import load_task

ap = argparse.ArgumentParser()
ap.add_argument("data"); ap.add_argument("repo"); ap.add_argument("task"); ap.add_argument("arm")
ap.add_argument("--environment", default="newWorktree")
ap.add_argument("--permission"); ap.add_argument("--model", default="opus"); ap.add_argument("--effort", default="high")
a = ap.parse_args()
task = load_task(a.task)
repo = os.path.realpath(a.repo)
cat = req(a.data, {"method": "getCatalog"})["value"]["catalog"]
proj = [p for p in cat["projects"] if p["repos"] and os.path.realpath(p["repos"][0]["path"]) == repo][0]
permission = a.permission or cat["settings"]["defaultPermission"]
if a.environment == "newWorktree":
    env = {"type": "newWorktree", "base": "main", "branch": None}
else:
    env = {"type": "localCheckout", "branch": "main", "createFrom": None}
setup = {"type": "session", "repo": proj["repos"][0]["path"], "environment": env, "permission": permission,
         "orchestrator": {"provider": "claude", "model": a.model, "effort": a.effort}, "planMode": False}
r = req(a.data, {"method": "createConversation", "kind": "session", "projectId": proj["id"], "title": None, "setup": setup})
conv = r["value"]["conversation"]["id"]
print("created", conv, permission, a.environment)
attachments = []
if task.get("image"):
    raw = open(task["image"], "rb").read()
    mime = mimetypes.guess_type(task["image"])[0] or "image/png"
    added = req(a.data, {"method": "addAttachment", "name": os.path.basename(task["image"]), "mime": mime,
                         "data": base64.b64encode(raw).decode(), "pasted": False})
    ref = added["value"]["attachment"]
    ref["inline"] = int(task["image-number"])
    attachments.append(ref)
t0 = int(time.time() * 1000)
r2 = req(a.data, {"method": "sendMessage", "conversationId": conv, "text": task["request"], "attachments": attachments,
                  "mentions": [], "steer": False})
os.makedirs(a.arm, exist_ok=True)
json.dump({"arm": "brigadier", "conversation": conv, "t0_ms": t0, "permission": permission, "environment": a.environment,
           "task": task["id"], "send": r2}, open(os.path.join(a.arm, "start.json"), "w"), indent=1)
print("sent", t0, json.dumps(r2)[:300])

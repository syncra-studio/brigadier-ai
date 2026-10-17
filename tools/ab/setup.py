#!/usr/bin/env python3
"""Onboards a fresh arm daemon and adds the arm's clone as a project.
usage: setup.py <data-dir> <repo> [--permission askForApproval|approveForMe|fullAccess]
                [--disable claude|codex] [--claude-workers MODEL]
Without --permission the daemon's own default stays (what a new user gets). Fable is hidden.
--claude-workers pins every kind of work but reviews to that Claude model (a routing rule
`only`), so both arms of a token A/B run Claude workers while reviews route as usual."""
import argparse, json, os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bipc import req

ap = argparse.ArgumentParser()
ap.add_argument("data"); ap.add_argument("repo")
ap.add_argument("--permission"); ap.add_argument("--disable", action="append", default=[])
ap.add_argument("--claude-workers")
a = ap.parse_args()
cat = req(a.data, {"method": "getCatalog"})["value"]["catalog"]
s = cat["settings"]
s["onboarded"] = True
if a.permission:
    s["defaultPermission"] = a.permission
s["hiddenModels"] = [{"provider": "claude", "id": i} for i in ("fable", "claude-fable-5", "claude-fable-5-1")]
s["disabledProviders"] = a.disable
if a.claude_workers:
    s["routingOverrides"] = [{"id": "ab-claude-workers", "effect": "only",
                              "target": {"type": "model", "provider": "claude", "id": a.claude_workers},
                              "categories": ["scout", "research", "implement", "merge", "verify", "operate"],
                              "areas": [], "projectId": None, "createdAtMs": int(time.time() * 1000)}]
print("settings", req(a.data, {"method": "updateSettings", "settings": s})["status"])
r = req(a.data, {"method": "addProject", "path": os.path.realpath(a.repo), "name": "", "init": False})
print(json.dumps(r)[:600])
# Trusted, as the user answers when they first enter the project; unanswered, it asks first.
project = r["value"]["project"]["id"]
print("trust", req(a.data, {"method": "setFolderTrust", "id": project, "path": None, "trusted": True})["status"])
print("defaultPermission", req(a.data, {"method": "getCatalog"})["value"]["catalog"]["settings"]["defaultPermission"])

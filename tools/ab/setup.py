#!/usr/bin/env python3
"""Onboards a fresh arm daemon and adds the arm's clone as a project.
usage: setup.py <data-dir> <repo> [--permission askForApproval|approveForMe|fullAccess]
                [--disable claude|codex]
Without --permission the daemon's own default stays (what a new user gets). Fable is hidden."""
import argparse, json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bipc import req

ap = argparse.ArgumentParser()
ap.add_argument("data"); ap.add_argument("repo")
ap.add_argument("--permission"); ap.add_argument("--disable", action="append", default=[])
a = ap.parse_args()
cat = req(a.data, {"method": "getCatalog"})["value"]["catalog"]
s = cat["settings"]
s["onboarded"] = True
if a.permission:
    s["defaultPermission"] = a.permission
s["hiddenModels"] = [{"provider": "claude", "id": i} for i in ("fable", "claude-fable-5", "claude-fable-5-1")]
s["disabledProviders"] = a.disable
print("settings", req(a.data, {"method": "updateSettings", "settings": s})["status"])
r = req(a.data, {"method": "addProject", "path": os.path.realpath(a.repo), "name": "", "init": False})
print(json.dumps(r)[:600])
print("defaultPermission", req(a.data, {"method": "getCatalog"})["value"]["catalog"]["settings"]["defaultPermission"])

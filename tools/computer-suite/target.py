#!/usr/bin/env python3
"""The dev-build target of the suite's dev-settings and dev-rename tasks: a second dev daemon on
its own data dir and the dev app on it, opened in the background, with a chat called "Suite
target". Its settings are set so the answer isn't the default: new sessions start on "Approve for
me". Nothing here touches the installed app, its daemon or its data.
usage: target.py start <dir> <app bundle>   (the daemon is started apart: startd.sh <dir>/data)
       target.py prepare <dir> <task> <trial dir>   writes the trial's setup.json
       target.py check <dir> <task> <trial dir>     the IPC half of the check, as JSON
       target.py stop <dir>                          quits the app it opened"""
import json, os, re, subprocess, sys, time
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "ab"))
from bipc import req

TITLE = "Suite target"
RENAMED = "Renamed by suite"
PERMISSION = "approveForMe"
HELPER = os.path.join(HERE, "..", "..", "target", "release", "brigadier-computer")


def value(r):
    if r is None or r.get("status") != "ok":
        raise SystemExit(f"target: {json.dumps(r)[:400]}")
    return r["value"]


def catalog(data):
    return value(req(data, {"method": "getCatalog"}))["catalog"]


def pids_of(binary):
    out = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True, text=True).stdout
    return {int(l.split(None, 1)[0]) for l in out.splitlines() if l.split(None, 1)[1:] and l.split(None, 1)[1].startswith(binary)}


def windows(pid):
    """The app's windows, from the helper's `apps` listing: `<name> pid <n>` lines, each followed
    by `  w<id> "<title>" <w>x<h> ...` lines."""
    out = subprocess.run([HELPER, "apps"], capture_output=True, text=True, check=True).stdout
    found, mine = [], False
    for line in out.splitlines():
        m = re.match(r'\s+w(\d+) "(.*)" (\d+)x(\d+)', line)
        if m and mine:
            found.append({"id": int(m.group(1)), "title": m.group(2), "size": [int(m.group(3)), int(m.group(4))],
                          "on_screen": "off screen" not in line})
        elif not m:
            mine = re.search(rf" pid {pid}\b", line) is not None
    return found


def start(d, bundle):
    data = os.path.join(d, "data")
    s = catalog(data)["settings"]
    s["onboarded"] = True
    s["defaultPermission"] = PERMISSION
    s["hiddenModels"] = [{"provider": "claude", "id": i} for i in ("fable", "claude-fable-5", "claude-fable-5-1")]
    value(req(data, {"method": "updateSettings", "settings": s}))
    conv = ensure_conversation(data)
    binary = os.path.join(bundle, "Contents", "MacOS", "brigadier")
    before = pids_of(binary)
    if len(sys.argv) > 4:  # an instance this script already opened on this data dir
        before = before - {int(sys.argv[4])}
    else:
        subprocess.run(["open", "-n", "-g", "--env", f"BRIGADIER_DATA_DIR={data}", "-a", bundle], check=True)
    pid = None
    for _ in range(120):
        new = pids_of(binary) - before
        if new:
            pid = new.pop()
            break
        time.sleep(0.5)
    if pid is None:
        raise SystemExit("target: the dev app didn't start")
    win = None
    for _ in range(120):
        ws = [w for w in windows(pid) if w.get("title")]
        if ws:
            win = ws[0]
            break
        time.sleep(0.5)
    if win is None:
        raise SystemExit(f"target: the dev app {pid} shows no window")
    json.dump({"pid": pid, "started": started(pid), "binary": binary, "window": win["id"], "title": win["title"],
               "conversation": conv, "bundle": bundle},
              open(os.path.join(d, "target.json"), "w"), indent=1)
    print(json.dumps({"pid": pid, "window": win, "conversation": conv}))


def ensure_conversation(data):
    for c in catalog(data)["conversations"]:
        if c.get("title") in (TITLE, RENAMED):
            return c["id"]
    r = value(req(data, {"method": "createConversation", "kind": "chat", "projectId": None, "title": TITLE,
                         "setup": None}))
    return r["conversation"]["id"]


def prepare(d, task, trial):
    """Resets what the task changes and writes its setup.json, as `suite setup` does."""
    data = os.path.join(d, "data")
    t = json.load(open(os.path.join(d, "target.json")))
    if task == "dev-rename":
        value(req(data, {"method": "renameConversation", "id": t["conversation"], "title": TITLE}))
    ws = [w for w in windows(t["pid"]) if w.get("id") == t["window"]]
    if not ws:
        raise SystemExit("target: the dev app's window is gone")
    os.makedirs(trial, exist_ok=True)
    json.dump({"task": task, "pid": t["pid"], "window": t["window"], "window_title": ws[0].get("title", ""),
               "ready_ms": int(time.time() * 1000)}, open(os.path.join(trial, "setup.json"), "w"), indent=1)


def check(d, task, trial):
    """What only the target's daemon knows: the chat's title, the default permission."""
    data = os.path.join(d, "data")
    t = json.load(open(os.path.join(d, "target.json")))
    out = {"task": task}
    if task == "dev-rename":
        convs = catalog(data)["conversations"]
        title = next((c.get("title") for c in convs if c["id"] == t["conversation"]), None)
        out.update(title=title, pass_=title == RENAMED)
    elif task == "dev-settings":
        perm = catalog(data)["settings"]["defaultPermission"]
        report = open(os.path.join(trial, "report.md")).read().lower() if os.path.exists(os.path.join(trial, "report.md")) else ""
        # The answer is the report's first paragraph; its verification may list every level it saw.
        report = report.strip().split("\n\n")[0]
        # The level as Settings words it (lib/setup.ts); the answer must name it and no other level.
        words = {"askForApproval": "ask for approval", "approveForMe": "approve for me", "fullAccess": "full access"}
        named = [k for k, w in words.items() if w in report]
        out.update(default_permission=perm, named=named, pass_=named == [perm])
    print(json.dumps(out))


def started(pid):
    """The process's start time and command as ps prints them, or None when it is gone."""
    r = subprocess.run(["ps", "-o", "lstart=,command=", "-p", str(pid)], capture_output=True, text=True)
    return r.stdout.strip() or None


def stop(d):
    """Ends the dev app this script opened: only while its pid is still that process (the same
    start time and command), never a later process that reused the pid."""
    t = json.load(open(os.path.join(d, "target.json")))
    now = started(t["pid"])
    if not t.get("started") or now != t["started"] or not now.split(None, 5)[-1].startswith(t["binary"]):
        print(f"target: pid {t['pid']} is no longer the dev app; nothing stopped")
        return
    os.kill(t["pid"], 15)


if __name__ == "__main__":
    cmd, d = sys.argv[1], sys.argv[2]
    {"start": lambda: start(d, sys.argv[3]), "prepare": lambda: prepare(d, sys.argv[3], sys.argv[4]),
     "check": lambda: check(d, sys.argv[3], sys.argv[4]), "stop": lambda: stop(d)}[cmd]()

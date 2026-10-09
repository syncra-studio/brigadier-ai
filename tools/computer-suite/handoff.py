#!/usr/bin/env python3
"""The live handoff check (§8 Phase 4): a GUI-heavy request in a plain dev session, worded as a
user would, goes to an `operate` worker. The session gets the orchestrator a new session starts on; the
request names no tool, kind or worker. The fixture is set up as the form trial; `request_check`
judges the end state of all seven steps from the fixture's log, and the form checker's verdict is
kept beside it (it counts the request's other steps as wrong targets).
usage: handoff.py <root> <out dir> | handoff.py --recheck <out dir>"""
import json, os, subprocess, sys, time
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "ab"))
from bipc import req
import run

REQUEST = ('On this Mac, the Target Range app is open. In its window: type Grace in Name and compiler '
           'in Notes, tick the 16 pt checkbox, set Level to 37, choose Gamma in the Letter menu, open the '
           'sheet and close it again, and select Row 173 in the table. Tell me when it\'s done.')


# The request's end state, by the fixture's snapshot keys; every other control keeps its first value.
WANT = {"name": "Grace", "notes": "compiler", "check-16": "on", "slider": "37", "popup": "Gamma",
        "table": "173", "sheet": "closed"}
START = {"check-8": "off", "check-12": "off", "check-24": "off", "password": "0", "stepper": "5",
         "tabs": "First"}


def request_check(log):
    """The whole request, from the fixture's own log: the last state snapshot, and the sheet opened
    before it closed."""
    events = [json.loads(l) for l in open(log) if l.strip()]
    snaps = [e for e in events if e["id"] == "state" and e["ev"] == "snapshot"]
    if not snaps:
        return {"pass": False, "notes": ["no state snapshot"]}
    st = json.loads(snaps[-1]["v"])
    notes = [f"{k} is {st.get(k)!r}, not {v!r}" for k, v in WANT.items() if st.get(k) != v]
    notes += [f"{k} changed to {st.get(k)!r}" for k, v in START.items() if st.get(k) != v]
    opened = [i for i, e in enumerate(events) if e["id"] == "sheet" and e["ev"] == "opened"]
    closed = [i for i, e in enumerate(events) if e["id"] == "sheet" and e["ev"] == "closed"]
    if not (opened and closed and closed[-1] > opened[0]):
        notes.append("the sheet wasn't opened and closed")
    others = sorted({e["id"] for e in events if e["id"].startswith(("dot-", "button-", "menu"))})
    if others:
        notes.append("other controls used: " + ", ".join(others))
    return {"pass": not notes, "notes": notes}


def recheck(out):
    f = os.path.join(out, "handoff.json")
    result = json.load(open(f))
    if "check" in result:  # Written before request_check existed.
        result["form_check"] = result.pop("check")
    result["request_check"] = request_check(os.path.join(out, "target", "fixture-log.jsonl"))
    json.dump(result, open(f, "w"), indent=1)
    print(json.dumps(result["request_check"], indent=1))


def wait(data, conv):
    """The task the thread delegated, once it ended and the thread is idle; or None once the thread
    went idle having done the work itself (it delegated nothing)."""
    deadline = time.time() + run.TASK_TIMEOUT_S
    started = False
    while time.time() < deadline:
        v = run.view(data, conv)
        task = v["tasks"][-1] if v["tasks"] else None
        idle = v["run"] in ("idle", "hibernated") and not v["queue"].get("items")
        started = started or not idle
        if idle and started and (task is None or task["state"] in run.END):
            return task, v, False
        time.sleep(3)
    return None, run.view(data, conv), True


def main():
    if sys.argv[1] == "--recheck":
        return recheck(sys.argv[2])
    root, out = sys.argv[1], sys.argv[2]
    data = os.path.join(root, "data")
    os.makedirs(out, exist_ok=True)
    scratch = os.path.join(out, "target")
    s = run.suite("setup", "form", scratch, "--seed", "7")
    if s.returncode:
        raise SystemExit(s.stderr)
    prep = json.load(open(os.path.join(scratch, "setup.json")))
    cat = run.ok(req(data, {"method": "getCatalog"}))["catalog"]
    proj = cat["projects"][0]
    setup = {"type": "session", "repo": proj["repos"][0]["path"],
             "environment": {"type": "localCheckout", "branch": "main", "createFrom": None},
             "permission": cat["settings"]["defaultPermission"], # What the composer starts a new session on when no default is set.
             "orchestrator": cat["settings"]["defaultOrchestrator"] or {"provider": "claude", "model": "opus", "effort": "high"},
             "planMode": False}
    conv = run.ok(req(data, {"method": "createConversation", "kind": "session", "projectId": proj["id"],
                             "title": "Handoff check", "setup": setup}))["conversation"]["id"]
    t0 = run.now_ms()
    text = REQUEST
    run.ok(req(data, {"method": "sendMessage", "conversationId": conv, "text": text, "attachments": [],
                      "mentions": [], "steer": False}))
    task, v, timed_out = wait(data, conv)
    recs = run.records(data, conv, task["id"]) if task else []
    with open(os.path.join(out, "records.jsonl"), "w") as f:
        for a in recs:
            f.write(a["record"] + "\n")
    c = run.suite("check", scratch, "--records", os.path.join(out, "records.jsonl"))
    run.suite("teardown", scratch)
    result = {"conversation": conv, "request": text, "t0_ms": t0, "t1_ms": run.now_ms(), "timed_out": timed_out,
              "orchestrator": setup["orchestrator"],
              "tasks": [{"title": t["title"], "kind": t["kind"], "state": t["state"],
                         "route": t["route"]["choice"], "target": t.get("target"), "end_state": t.get("endState")}
                        for t in v["tasks"]],
              "handed_to_operate": any(t["kind"] == "operate" for t in v["tasks"]),
              "request_check": request_check(prep["log"]), "form_check": c.stdout,
              "reply": run.last_reply(v)}
    json.dump(result, open(os.path.join(out, "handoff.json"), "w"), indent=1)
    print(json.dumps({k: result[k] for k in ("handed_to_operate", "tasks", "request_check", "reply")}, indent=1))


if __name__ == "__main__":
    main()

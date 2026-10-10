#!/usr/bin/env python3
"""The computer-use suite with a model in the loop (COMPUTER-USE-PLAN §7, §8 Phase 4).

One run is one provider's pass over the tasks, in a session of its own on the arm daemon. Per
task: `suite setup` (or target.py for the dev build), a message that has the thread call
`delegate_task` with kind operate and exact arguments, a wait for the task and the thread's relay
turn, then the broker's records, the report, `suite check`, `suite teardown`, the usage and the
model calls from the transcripts, and the shortcut audit.

usage: run.py <root> <provider> <run> [task ...] [--model ID] [--seed N]
  <root>/data      the arm daemon's data dir (tools/ab/startd.sh, onboarded by tools/ab/setup.py)
  <root>/rec       tools/ab/recorder.py's events; <root>/transcripts its hard-linked transcripts
  <root>/target    the dev-build target (target.py), for the dev-* tasks
  <root>/<provider>-<run>/<task>/   one trial: setup.json, records.jsonl, report.md, result.json
The F1 monitor (`suite watch`) runs for the whole run: <root>/<provider>-<run>/focus.jsonl."""
import argparse, json, os, sqlite3, subprocess, sys, time
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "ab"))
sys.path.insert(0, HERE)
from bipc import req
import calls

WT = os.path.realpath(os.path.join(HERE, "..", ".."))
HELPER = os.path.join(WT, "target", "release", "brigadier-computer")
END = {"reported", "done", "rejected", "stopped", "failed", "landed"}
TASK_TIMEOUT_S = 900


def now_ms():
    return int(time.time() * 1000)


def ok(r):
    if r is None or r.get("status") != "ok":
        raise RuntimeError(json.dumps(r)[:600])
    return r["value"]


def suite(*args):
    return subprocess.run([HELPER, "suite", *args], capture_output=True, text=True)


def view(data, conv):
    return ok(req(data, {"method": "getConversation", "id": conv, "limit": 20}))["view"]


def session(root, provider, run, out):
    """The run's session: created once, kept in conversation.json."""
    f = os.path.join(out, "conversation.json")
    if os.path.exists(f):
        return json.load(open(f))["conversation"]
    data = os.path.join(root, "data")
    cat = ok(req(data, {"method": "getCatalog"}))["catalog"]
    proj = cat["projects"][0]
    setup = {"type": "session", "repo": proj["repos"][0]["path"],
             "environment": {"type": "localCheckout", "branch": "main", "createFrom": None},
             "permission": "fullAccess",
             # The thread only relays: the smallest model, low effort.
             "orchestrator": {"provider": "claude", "model": "sonnet", "effort": "low"}, "planMode": False}
    conv = ok(req(data, {"method": "createConversation", "kind": "session", "projectId": proj["id"],
                         "title": f"Suite {provider} {run}", "setup": setup}))["conversation"]["id"]
    json.dump({"conversation": conv, "created_ms": now_ms()}, open(f, "w"), indent=1)
    return conv


def message(t, prep, provider, model):
    """The thread's instruction: one delegate_task call with these exact arguments."""
    target = (f'The "{prep["window_title"]}" window (window id {prep["window"]}, pid {prep["pid"]}), '
              "already open on this Mac: don't launch or quit anything.")
    args = {"title": t["id"], "kind": "operate", "provider": provider, "effort": "medium",
            "target": target, "end_state": t["end_state"], "spec": t["brief"]}
    if model:
        args["model"] = model
    return ("This is a computer-use suite trial. Call delegate_task once, now, with exactly these arguments "
            "and nothing added or changed:\n```json\n" + json.dumps(args, indent=1) + "\n```\n"
            "Don't do the task yourself and don't use the computer tools. When the worker reports, "
            "reply with one line: whether it says the end state holds.")


def wait_task(data, conv, before):
    """The task the thread delegated for this trial (the newest one not in `before`), once it has
    ended and the thread is idle. The thread may word the title its own way."""
    deadline = time.time() + TASK_TIMEOUT_S
    task = None
    while time.time() < deadline:
        v = view(data, conv)
        mine = [t for t in v["tasks"] if t["id"] not in before]
        task = mine[-1] if mine else None
        idle = v["run"] in ("idle", "hibernated")
        if task and task["state"] in END and idle and not v["queue"].get("items"):
            return task, v, False
        time.sleep(3)
    return task, view(data, conv), True


def records(data, conv, task_id):
    """The broker's records of the task, oldest first, as the engine wrote them."""
    out, before = [], None
    while True:
        page = ok(req(data, {"method": "listComputerActions", "conversationId": conv, "taskId": task_id,
                             "before": before, "limit": 200}))["page"]
        out = page["actions"] + out
        if page.get("earlier") is None:
            return out
        before = page["earlier"]


def report_text(task):
    r = task.get("report") or {}
    parts = [r.get("summary") or ""]
    for k in ("verification", "doneWhen", "openQuestions", "risks", "needsUser"):
        if r.get(k):
            parts.append(f"{k}:\n" + "\n".join(f"- {x}" for x in r[k]))
    return "\n\n".join(p for p in parts if p).strip()


def usage(data_dir, conv, task_id, t0, t1):
    """turn_usage of the task (the worker) and of the thread in the trial's window."""
    db = sqlite3.connect(f"file:{os.path.join(data_dir, 'routing.sqlite')}?mode=ro", uri=True)
    q = ("SELECT provider, model, COUNT(*), SUM(input), SUM(cached_input), SUM(cache_write), SUM(output) "
         "FROM turn_usage WHERE {} GROUP BY provider, model")
    def rows(where, args):
        return [dict(zip(("provider", "model", "turns", "input_uncached", "cache_read", "cache_write", "output"), r))
                for r in db.execute(q.format(where), args)]
    return {"worker": rows("task_id = ?", (task_id,)),
            "thread": rows("conversation_id = ? AND task_id IS NULL AND at_ms BETWEEN ? AND ?", (conv, t0, t1))}


def native_ids(root, task_id):
    """The CLI sessions the task's attempts ran in, from the recorded events."""
    ids = []
    for line in open(os.path.join(root, "rec", "events.jsonl")):
        if task_id not in line or "sessionStarted" not in line:
            continue
        ev = json.loads(line)["event"]
        if ev.get("type") == "workerEvent" and ev.get("taskId") == task_id:
            nid = ev["event"].get("nativeId")
            if nid and nid not in ids:
                ids.append(nid)
    return ids


def trial(root, out, t, provider, model, seed, conv):
    data = os.path.join(root, "data")
    d = os.path.join(out, t["id"].replace(" ", "-"))
    os.makedirs(d, exist_ok=True)
    scratch = os.path.join(d, "target")
    if t["setup"]["kind"] == "dev_app":
        # The dev app's window may have been closed by the person at the Mac: that trial isn't run.
        s = subprocess.run([sys.executable, os.path.join(HERE, "target.py"), "prepare", os.path.join(root, "target"),
                            t["id"], scratch], capture_output=True, text=True)
        if s.returncode != 0:
            return {"task": t["id"], "error": "setup failed: " + s.stderr[-600:]}
    else:
        s = suite("setup", t["id"], scratch, "--seed", str(seed))
        if s.returncode != 0:
            return {"task": t["id"], "error": "setup failed: " + s.stderr[-600:]}
    prep = json.load(open(os.path.join(scratch, "setup.json")))
    before = {x["id"] for x in view(data, conv)["tasks"]}
    t0 = now_ms()
    ok(req(data, {"method": "sendMessage", "conversationId": conv, "text": message(t, prep, provider, model),
                  "attachments": [], "mentions": [], "steer": False}))
    task, v, timed_out = wait_task(data, conv, before)
    t1 = now_ms()
    result = {"task": t["id"], "provider_asked": provider, "seed": seed, "t0_ms": t0, "t1_ms": t1,
              "timed_out": timed_out, "pid": prep["pid"], "window": prep["window"]}
    if task is None:
        result["error"] = "the thread delegated no task"
        result["thread_reply"] = last_reply(v)
        teardown(t, scratch)
        return result
    if timed_out:
        ok(req(data, {"method": "stopTask", "taskId": task["id"]}))
    recs = records(data, conv, task["id"])
    with open(os.path.join(d, "records.jsonl"), "w") as f:
        for a in recs:
            f.write(a["record"] + "\n")
    json.dump(recs, open(os.path.join(d, "actions.json"), "w"), indent=1)
    open(os.path.join(d, "report.md"), "w").write(report_text(task))
    json.dump(task, open(os.path.join(d, "task.json"), "w"), indent=1)
    if t["setup"]["kind"] == "dev_app":
        open(os.path.join(scratch, "report.md"), "w").write(report_text(task))
    c = suite("check", scratch, "--records", os.path.join(d, "records.jsonl"), "--report", os.path.join(d, "report.md"))
    try:
        verdict = json.loads(c.stdout)
    except json.JSONDecodeError:
        verdict = {"pass": False, "notes": ["check failed: " + (c.stderr or c.stdout)[-600:]]}
    if t["setup"]["kind"] == "dev_app":
        ipc = json.loads(subprocess.run([sys.executable, os.path.join(HERE, "target.py"), "check",
                                         os.path.join(root, "target"), t["id"], scratch],
                                        capture_output=True, text=True, check=True).stdout)
        verdict["ipc"] = ipc
        if not ipc.get("pass_"):
            verdict["pass"] = False
            verdict.setdefault("notes", []).append(f"the target's daemon disagrees: {ipc}")
    teardown(t, scratch)
    attempts = task.get("attempts", [])
    result.update(
        task_id=task["id"], state=task["state"], verdict=verdict,
        attempts=[{"provider": a["route"]["choice"]["provider"], "model": a["route"]["choice"].get("model"),
                   "effort": a["route"]["choice"].get("effort"),
                   "start_ms": a.get("startedAtMs"), "end_ms": a.get("endedAtMs"), "end": a.get("end")}
                  for a in attempts],
        batches=len({a["batch"] for a in recs}), actions=len(recs),
        worker_ms=(attempts[-1].get("endedAtMs") or t1) - attempts[0]["startedAtMs"] if attempts else None,
        wall_ms=t1 - t0, usage=usage(data, conv, task["id"], t0, t1),
        sessions=native_ids(root, task["id"]), thread_reply=last_reply(v))
    result["forbidden"] = forbidden(root, scratch, prep)
    recount(root, result)
    json.dump(result, open(os.path.join(d, "result.json"), "w"), indent=1)
    return result


def forbidden(root, scratch, prep):
    """What a worker may read but never write: the trial's files and log, and the daemon's own
    state (its data dir, but not the scratch folders workers run in)."""
    data = os.path.join(root, "data")
    own = [os.path.join(data, n) for n in sorted(os.listdir(data)) if n not in ("scratch", "worktrees")]
    return [scratch, os.path.realpath(scratch), prep.get("log") or "", *prep.get("files", {}),
            prep.get("browser_profile") or "", *own]


def recount(root, r):
    """Model calls and the shortcut audit from the transcripts; again at settle, when the last
    requests of the turn are in."""
    tr = os.path.join(root, "transcripts")
    r["calls"] = calls.count(tr, r["sessions"])
    r["audit"] = calls.audit(tr, r["sessions"], r["forbidden"])
    v = r["verdict"]
    if r["audit"]["shortcuts"] and not any(n.startswith("shortcut:") for n in v.get("notes", [])):
        v["pass"] = False
        v.setdefault("notes", []).append("shortcut: " + "; ".join(r["audit"]["shortcuts"])[:400])


def last_reply(v):
    msgs = v["messages"]["messages"] if isinstance(v.get("messages"), dict) else []
    for m in reversed(msgs):
        if m.get("role") == "assistant":
            return (m.get("text") or "")[:400]
    return None


def teardown(t, scratch):
    if t["setup"]["kind"] != "dev_app":
        suite("teardown", scratch)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root"); ap.add_argument("provider"); ap.add_argument("run", type=int)
    ap.add_argument("tasks", nargs="*"); ap.add_argument("--model"); ap.add_argument("--seed", type=int)
    a = ap.parse_args()
    tasks = json.loads(suite("tasks").stdout)
    if a.tasks:
        tasks = [t for t in tasks if t["id"] in a.tasks or t["id"].replace(" ", "-") in a.tasks]
    out = os.path.join(a.root, f"{a.provider}-{a.run}")
    os.makedirs(out, exist_ok=True)
    conv = session(a.root, a.provider, a.run, out)
    watch = subprocess.Popen([HELPER, "suite", "watch", os.path.join(out, f"focus-{now_ms()}.jsonl")],
                             stdin=subprocess.PIPE)
    done = []
    try:
        for t in tasks:
            r = trial(a.root, out, t, a.provider, a.model, a.seed if a.seed is not None else a.run, conv)
            v = r.get("verdict", {})
            print(json.dumps({"task": r["task"], "pass": v.get("pass"), "state": r.get("state"),
                              "model": [x["model"] for x in r.get("attempts", [])], "batches": r.get("batches"),
                              "calls": (r.get("calls") or {}).get("model_calls"), "wall_s": round(r.get("wall_ms", 0) / 1000),
                              "notes": v.get("notes"), "error": r.get("error")}), flush=True)
            done.append(r)
    finally:
        watch.stdin.close()
        watch.wait(timeout=10)
        settle(a.root, out, conv, done)


def settle(root, out, conv, done):
    """Usage again once the last turns have written theirs: a worker's row lands when its turn
    ends, after its report. A trial's thread window runs to the next trial's start."""
    time.sleep(20)
    for i, r in enumerate(done):
        if "task_id" not in r:
            continue
        # The next trial that ran (one whose setup failed has no start).
        end = next((x["t0_ms"] for x in done[i + 1:] if "t0_ms" in x), now_ms())
        r["usage"] = usage(os.path.join(root, "data"), conv, r["task_id"], r["t0_ms"], end)
        recount(root, r)
        json.dump(r, open(os.path.join(out, r["task"].replace(" ", "-"), "result.json"), "w"), indent=1)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""The real small-window runs of BUILD-PLAN D2.3 and D2.3b: one turn that reads far more than the
model's window holds, on a model whose window is small, with Brigadier's real thresholds (no debug
overrides). The thread (or one worker) must be asked to commit, then be stopped at the hard line and
continued in a fresh session (a mid-turn rebirth for the thread, a hand-off for a worker), and its
CLI must never compact.

A root is a V2 root (`run.py template`, same fixture and settings) made with `--model`:
  window.py template --root R --provider claude --model claude-sonnet-4-6   (a 200k window)
  window.py template --root R --provider codex --model gpt-6-luna           (Codex's smallest, 272k)
  window.py run --root R --role thread|worker [--n 1] [--files 24]
  window.py score RUN_DIR                    (rescored from the run directory alone)
  run.py cleanup --root R [--export DIR]

Each run (`runs/window-<role>-<provider>-<n>/`) keeps the record, the orchestrator log, start.json
and score.json. The score says, from the record alone:
  steers           the thread's mid-turn "commit now" steers / the worker's wrap-up steers
  stopped          the thread's StoppedForRebirth entries / the worker's hard-line or refused-compaction stops
  rebirths_mid     the thread's mid-turn rebirths
  worker_sessions  the workers' CLI sessions (a hand-off adds one); handovers, their notices
  compactions      CompactionStarted events, of the thread and every worker (must be 0)
  window           the largest context window any session reported
  answered         the request ended done, and the conversation names every file's CHECK word
Pass: no compaction and at least one steer; the thread stopped and reborn mid-turn, or the worker
continued in a fresh session; answered.
Nothing here touches the installed app, its daemon or its data."""
import argparse, importlib.util, json, os, random, re, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("v2run", os.path.join(HERE, "run.py"))
v2 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(v2)
v3run, req, log, now_ms = v2.v3run, v2.req, v2.log, v2.now_ms

WORDS = ("amber basil cedar delta ember fable garnet harbor indigo juniper kestrel lumen meadow nectar "
         "onyx pepper quartz raven saffron tundra umber velvet willow xenon yarrow zephyr").split()
TIMEOUT_S = 3 * 3600


def check_word(i):
    return f"CHECK-{i:02d}-{random.Random(i).randrange(16**6):06x}"


def write_files(ws, count):
    """`bulk/01.txt` … : about 40 KB each (roughly 10k tokens), its CHECK word on its last line."""
    d = os.path.join(ws, "bulk")
    os.makedirs(d, exist_ok=True)
    for i in range(1, count + 1):
        rnd = random.Random(1000 + i)
        lines = [f"file {i:02d} line {n:04d}: " + " ".join(rnd.choice(WORDS) for _ in range(10)) for n in range(1, 520)]
        lines.append(check_word(i))
        open(os.path.join(d, f"{i:02d}.txt"), "w").write("\n".join(lines) + "\n")
    # Committed on the session's branch: a worker's checkout is made from its committed tip.
    git = ["git", "-C", ws, "-c", "user.name=V2 harness", "-c", "user.email=v2@example.invalid", "-c", "commit.gpgsign=false"]
    subprocess.run(git + ["add", "bulk"], check=True, capture_output=True)
    subprocess.run(git + ["commit", "-q", "-m", "Add the bulk files to read"], check=True, capture_output=True)
    return d


def ask(role, count):
    files = ", ".join(f"bulk/{i:02d}.txt" for i in range(1, count + 1))
    task = (f"Read every one of these files completely, one read per file, in this order: {files}. Read each "
            "whole file (all of its lines; for a shell read use `cat` on the whole file, never head, tail or "
            "grep). Don't summarise between reads and don't skip any. Then answer with one line per file: its "
            "name and the CHECK-… word on its last line. Change no files and commit nothing.")
    if role == "thread":
        return "Do this yourself in this conversation, without starting any worker. " + task
    return ("Start exactly one worker for this and wait for its report; don't read the files yourself. Give it "
            "this task word for word: " + task + " When it reports, answer me with its list.")


def template(a):
    v2.MODELS[a.provider] = a.model
    v2.template(a)
    build = json.load(open(os.path.join(a.root, "build.json")))
    build["model"] = a.model
    json.dump(build, open(os.path.join(a.root, "build.json"), "w"), indent=1)


def one(root, role, n, count):
    build = json.load(open(os.path.join(root, "build.json")))
    provider, model = build["provider"], build["model"]
    v2.MODELS[provider] = model
    rid_ = f"window-{role}-{provider}-{n}"
    run = os.path.join(root, "runs", rid_)
    if os.path.exists(os.path.join(run, "score.json")):
        log(rid_, "scored already"); return
    if os.path.exists(run):
        v3run.safe_rmtree(root, run)
    os.makedirs(run)
    v3run.restore(root, "seeded")
    arm = v2.Arm(root, os.path.join(run, "rec"), run, "")
    start = {"role": role, "provider": provider, "model": model, "files": count, "n": n, "build": build,
             "check_words": [check_word(i) for i in range(1, count + 1)], "requests": []}
    try:
        time.sleep(5)
        conv = v2.create_conversation(arm.data, arm.repo, provider)
        start["conversation"] = conv
        deadline = time.time() + TIMEOUT_S
        # A first short message makes the workspace and tells Brigadier the model's window.
        t, rid = v3run.send(arm.data, conv, "Say ready, nothing else. " + v2.v3lib.SAFETY)
        start["requests"].append(rid); start["t0_ms"] = t
        if v3run.wait_request(arm.events, conv, rid, deadline) is None:
            raise SystemExit("the first message never ended")
        v3run.settle(arm.events, conv, deadline)
        ws = v2.workspace(arm.events, conv)
        start["workspace"] = ws
        start["bulk"] = write_files(ws, count)
        t, rid = v3run.send(arm.data, conv, ask(role, count) + " " + v2.v3lib.SAFETY)
        start["requests"].append(rid); start["big_request"] = rid; start["sent_ms"] = t
        json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
        r = v3run.wait_request(arm.events, conv, rid, deadline)
        start["timed_out"] = r is None
        start["settled_quietly"] = False if r is None else v3run.settle(arm.events, conv, deadline + v3run.SETTLE_MAX_S)
        entries = v2.orchestrator_log(arm.data, conv)
        with open(os.path.join(run, "orchestrator.jsonl"), "w") as f:
            for e in entries:
                f.write(json.dumps(e) + "\n")
        json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
    finally:
        arm.close()
    v3run.backup(os.path.join(arm.data, "routing.sqlite"), v3run.ensure(os.path.join(run, "data")) + "/routing.sqlite")
    v3run.after(run)
    s = score(run)
    json.dump(s, open(os.path.join(run, "score.json"), "w"), indent=1, sort_keys=True)
    with open(os.path.join(root, "results.jsonl"), "a") as f:
        f.write(json.dumps({"run": rid_, **s}, sort_keys=True) + "\n")
    log(rid_, "PASS" if s["pass"] else "FAIL", json.dumps(s)[:400])


def walk(value):
    """Every string inside a JSON value."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for v in value.values():
            yield from walk(v)
    elif isinstance(value, list):
        for v in value:
            yield from walk(v)


def score(run):
    start = json.load(open(os.path.join(run, "start.json")))
    role, conv, big = start["role"], start["conversation"], start.get("big_request")
    entries = [json.loads(line) for line in open(os.path.join(run, "orchestrator.jsonl"))] \
        if os.path.exists(os.path.join(run, "orchestrator.jsonl")) else []
    events = list(v3run.read_events(os.path.join(run, "rec", "events.jsonl")))
    s = {"role": role, "provider": start["provider"], "model": start["model"], "compactions": 0, "window": None,
         "steers": 0, "stopped": 0, "rebirths_mid": 0, "stop_reasons": [], "worker_sessions": 0, "handovers": 0}

    def note_window(ev):
        w = ev.get("windowTokens")
        if isinstance(w, int):
            s["window"] = max(s["window"] or 0, w)

    for e in entries:
        entry = e["entry"]
        t = entry.get("type")
        ev = entry.get("event") or {}
        if t == "provider":
            if ev.get("type") == "compactionStarted":
                s["compactions"] += 1
            if ev.get("type") == "contextSize":
                note_window(ev)
        if t == "stoppedForRebirth":
            s["stopped"] += 1; s["stop_reasons"].append(entry.get("reason"))
        if t == "rebirth" and (entry.get("record") or {}).get("trigger") == "midTurn":
            s["rebirths_mid"] += 1
        if role == "thread" and any("Your context is now about" in text for text in walk(entry)):
            s["steers"] += 1
    sessions = set()
    for e in events:
        if e.get("type") != "workerEvent":
            continue
        ev = e.get("event") or {}
        if ev.get("type") == "compactionStarted":
            s["compactions"] += 1
        if ev.get("type") == "contextSize":
            note_window(ev)
        if ev.get("type") == "sessionStarted":
            sessions.add(ev.get("nativeId"))
        if role == "worker" and ev.get("type") == "notice":
            message = ev.get("message") or ""
            if "stopped the turn there" in message or "began compacting its context" in message:
                s["stopped"] += 1
            if "asked the worker to end its turn with a handoff note" in message:
                s["steers"] += 1
            if message.startswith("Continued in a fresh session of the same model"):
                s["handovers"] += 1
    s["worker_sessions"] = len(sessions)
    if role == "worker":
        s["rebirths_mid"] = max(0, len(sessions) - 1)
    reqs, _, _ = v3run.conv_state(os.path.join(run, "rec", "events.jsonl"), conv)
    r = reqs.get(big) or {}
    s["request_state"] = (r.get("state") or {}).get("type")
    answer = " ".join(text for e in events if e.get("type") == "messageAppended"
                      and e["message"].get("conversationId") == conv for text in walk(e["message"]))
    words = start.get("check_words", [])
    s["check_words_named"] = sum(1 for w in words if w in answer)
    s["answered"] = s["request_state"] == "done" and s["check_words_named"] == len(words)
    s["timed_out"] = start.get("timed_out")
    # The thread is asked to commit and goes on, so it is stopped at the hard line and reborn
    # mid-turn (D2.3b). A worker asked to wrap up may end its turn with a note and be handed off
    # then, or go on and be stopped at the hard line: either way a fresh session continues (D2.3).
    continued = s["rebirths_mid"] >= 1 if role == "thread" else s["worker_sessions"] >= 2
    stopped = s["stopped"] >= 1 if role == "thread" else True
    s["pass"] = s["compactions"] == 0 and s["steers"] >= 1 and stopped and continued and s["answered"]
    return s


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("template"); t.add_argument("--root", required=True); t.add_argument("--tag")
    t.add_argument("--provider", choices=sorted(v2.MODELS), required=True); t.add_argument("--model", required=True)
    r = sub.add_parser("run"); r.add_argument("--root", required=True)
    r.add_argument("--role", choices=["thread", "worker"], required=True)
    r.add_argument("--n", type=int, default=1); r.add_argument("--files", type=int, default=24)
    sc = sub.add_parser("score"); sc.add_argument("run")
    a = ap.parse_args()
    if a.cmd == "template":
        template(a)
    elif a.cmd == "run":
        root = v3run.check_root(a.root)
        try:
            one(root, a.role, a.n, a.files)
        except Exception as err:  # noqa: BLE001
            log("HARNESS ERROR", repr(err))
            with open(os.path.join(root, "results.jsonl"), "a") as f:
                f.write(json.dumps({"run": f"window-{a.role}-{a.n}", "pass": False, "why": f"harness error: {err!r}"}) + "\n")
            sys.exit(1)
    else:
        print(json.dumps(score(a.run), indent=1, sort_keys=True))


if __name__ == "__main__":
    main()

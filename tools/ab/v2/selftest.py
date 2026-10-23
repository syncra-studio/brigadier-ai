#!/usr/bin/env python3
"""The V2 scorer's rules on synthetic runs (no daemon, no model): each case builds a run directory
the way run.py leaves one, scores it, checks the verdicts, and checks that rescoring is
byte-identical. With a root, also rescores every real run under it and compares with its
score.json.

usage: selftest.py [<harness-root>]"""
import json, os, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import score as scorer  # noqa: E402

CONV = "conv-1"
GOOD = {
    1: 'Every new exported function gets a JSDoc comment whose first line starts with "Since 0.5:"; in src/exporters/ it starts with "Exporter since 0.5:" instead.',
    2: "Invoice numbers stay zero-padded to 6 digits, because the accounting import rejects shorter ones.",
    3: "4e1d9b7c2a03",
    4: "src/invoice.ts:57",
    5: "4500 ms",
    6: "import-check: row 17: column tax_region is empty (batch B-2291)",
    7: "Caching catalogFromStore results in a module-level Map: it broke the storage tests.",
    8: "VT-88213",
    9: "Unverified: it is a worker's claim; writeDataFile calls writeFileSync directly.",
}


def battery():
    from v2lib import load_battery  # noqa: E402
    return load_battery("thread")


def make(dirpath, *, answers, seam="restart", killed=2, checks=None, timed_out=False):
    b = battery()
    os.makedirs(os.path.join(dirpath, "rec"), exist_ok=True)
    shutil.copyfile(b["path"], os.path.join(dirpath, "battery.md"))
    steps = [s["id"] for s in b["steps"]]
    sent = [1_000_000 + i * 100_000 for i in range(len(steps))]
    requests = [f"req-{s}" for s in steps]
    json.dump({"battery": "thread", "seam": seam, "provider": "claude", "label": "test", "n": 1, "conversation": CONV,
               "t0_ms": sent[0], "steps": steps, "requests": requests, "sent_ms": sent,
               "seams": [{"killed_pid": 100 + i} for i in range(killed)], "timed_out": timed_out}, open(os.path.join(dirpath, "start.json"), "w"))
    seq, events = 0, []
    for s, rid, t in zip(steps, requests, sent):
        seq += 1
        events.append({"seq": seq, "atMs": t + 50_000, "event": {"type": "requestUpdated", "request": {
            "id": rid, "conversationId": CONV, "state": {"type": "done"}, "endedAtMs": t + 50_000}}})
        if s == "p1":
            seq += 1
            text = "\n".join(f"{n}. {a}" for n, a in sorted(answers.items()))
            events.append({"seq": seq, "atMs": t + 40_000, "event": {"type": "messageAppended", "message": {
                "id": f"m-{s}", "conversationId": CONV, "role": "assistant", "requestId": rid, "text": text, "seq": seq}}})
    with open(os.path.join(dirpath, "rec", "events.jsonl"), "w") as f:
        for e in events:
            f.write(json.dumps(e) + "\n")
    open(os.path.join(dirpath, "orchestrator.jsonl"), "w").close()
    json.dump(checks or {"e1": {"rc": 0, "result": {"constraint": True, "exception": True, "works": True}},
                         "e2": {"rc": 0, "result": {"ruled_out": True, "found": []}}},
              open(os.path.join(dirpath, "checks.json"), "w"))
    json.dump([], open(os.path.join(dirpath, "manifest.json"), "w"))


CASES = [
    ("all pass", dict(answers=GOOD),
     lambda s: s["all_pass"] and s["valid"] and s["completed"] and s["seams"]["forced"] == 2),
    ("a lost item said unknown, another guessed",
     dict(answers={**GOOD, 3: "unknown", 5: "about 3 seconds"}),
     lambda s: s["by_item"]["hash"] == "unknown" and s["by_item"]["units"] == "fail"
     and s["unknown_instead_of_guess"] == {"lost": 2, "unknown": 1, "guessed": 1, "score": 0.5} and not s["all_pass"]),
    ("the exception forgotten in the edit",
     dict(answers=GOOD, checks={"e1": {"rc": 1, "result": {"constraint": True, "exception": False}},
                                "e2": {"rc": 0, "result": {"ruled_out": True}}}),
     lambda s: s["by_item"]["exception"] == "fail" and s["by_item"]["constraint"] == "pass"),
    ("the ruled-out approach retried",
     dict(answers=GOOD, checks={"e1": {"rc": 0, "result": {"constraint": True, "exception": True}},
                                "e2": {"rc": 1, "result": {"ruled_out": False}}}),
     lambda s: s["by_item"]["ruled_out"] == "fail"),
    ("the false learned line claimed verified",
     dict(answers={**GOOD, 9: "Verified: it writes through a temp file and a rename."}),
     lambda s: s["by_item"]["false_learned"] == "fail"),
    ("two restarts: valid",
     dict(answers=GOOD),
     lambda s: s["valid"] and s["all_pass"] and s["seams"]["forced"] == 2),
    ("one restart only: invalid",
     dict(answers=GOOD, killed=1),
     lambda s: not s["valid"] and not s["all_pass"] and s["seams"]["forced"] == 1),
    ("no seam is a valid control",
     dict(answers=GOOD, seam="none", killed=0),
     lambda s: s["valid"] and s["all_pass"]),
    ("a timed-out run is not completed",
     dict(answers=GOOD, timed_out=True),
     lambda s: not s["completed"] and not s["all_pass"]),
]


def app_servers():
    """run.py's kill seam finds a Codex thread's app-server by its flags (#116): the thread's,
    not a worker's or a review's, as crates/providers/src/codex/mod.rs builds them."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("v2run", os.path.join(HERE, "run.py"))
    v2run = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(v2run)
    base = "/bin/codex app-server" + "".join(f" --disable {f}" for f in
                                             ("plugins", "apps", "hooks", "computer_use", "browser_use", "memories"))
    thread = base + "".join(f" --disable {f}" for f in ("multi_agent", "multi_agent_v2", "goals", "sleep_tool")) + \
        " -c agents.enabled=false -c skills.include_instructions=false -c notify=[]"
    review = base + "".join(f" --disable {f}" for f in ("view_image", "image_generation", "multi_agent", "multi_agent_v2",
                                                          "goals", "sleep_tool", "shell_tool", "unified_exec")) + " -c notify=[]"
    worker = base + " -c notify=[] -c allow_login_shell=false"
    return [v2run.thread_app_server(c) for c in (thread, review, worker)] == [True, False, False]


def main():
    failed = 0
    tmp = tempfile.mkdtemp(prefix="v2-selftest-")
    try:
        for i, (name, kw, ok) in enumerate(CASES):
            d = os.path.join(tmp, f"case{i}")
            make(d, **kw)
            first = json.dumps(scorer.score(d), sort_keys=True)
            again = subprocess.run([sys.executable, os.path.join(HERE, "score.py"), d], check=True, capture_output=True, text=True).stdout
            same = json.loads(again) == json.loads(first)
            good = ok(json.loads(first)) and same
            failed += not good
            print("ok  " if good else "FAIL", name, "" if same else "(rescoring differs)")
            if not good:
                print(json.dumps({k: v for k, v in json.loads(first).items() if k != "items"}, indent=1))
        good = app_servers()
        failed += not good
        print("ok  " if good else "FAIL", "the kill seam finds a Codex thread's app-server, not a worker's or a review's")
        for root in sys.argv[1:]:
            for run in sorted(os.listdir(os.path.join(root, "runs"))):
                d = os.path.join(root, "runs", run)
                if os.path.exists(os.path.join(d, "score.json")):
                    same = scorer.score(d) == json.load(open(os.path.join(d, "score.json")))
                    failed += not same
                    print("ok  " if same else "FAIL", "rescore", run)
    finally:
        shutil.rmtree(tmp)
    print("selftest:", "FAILED" if failed else "passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

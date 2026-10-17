#!/usr/bin/env python3
"""The A10 scorer's rules on synthetic runs (no daemon, no model): each case builds a run directory
the way a10.py leaves one (with a small routing.sqlite), scores it, checks the verdicts, and checks
that rescoring is byte-identical. With a root, also rescores every real A10 run under it and compares
with its score.json.

usage: a10selftest.py [<harness-root>]"""
import json, os, shutil, sqlite3, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import a10score as scorer  # noqa: E402
from v2lib import load_battery  # noqa: E402

GOOD = scorer.load("v2selftest", os.path.join(HERE, "selftest.py")).GOOD

CONV = "conv-1"
ASKS = {"k1": "The default is set at src/cli.ts:64: 30 days.", "k2": "src/tax.ts:9 sets QC's standard rate to 0.15.",
        "l1": "src/services.ts:7: INVOICE_PREFIX is 'INV-'.", "l2": "src/invoice.ts:74: COL_NAME = 24 characters."}
STEP_MS = 100_000


def make(dirpath, *, p1=None, p2=None, reborn_after=("b03", "b08", "b15"), asks=None, checks=None):
    b = load_battery("a10")
    os.makedirs(os.path.join(dirpath, "rec"), exist_ok=True)
    os.makedirs(os.path.join(dirpath, "briefings"), exist_ok=True)
    os.makedirs(os.path.join(dirpath, "data"), exist_ok=True)
    shutil.copyfile(b["path"], os.path.join(dirpath, "battery.md"))
    steps = [s["id"] for s in b["steps"]]
    sent = [1_000_000 + i * STEP_MS for i in range(len(steps))]
    ended = [t + 50_000 for t in sent]
    requests = [f"req-{s}" for s in steps]
    contexts = [{"step": s, "used": 30_000 + 10_000 * i, "window": 1_000_000, "thresholds": {"prepareTokens": 200_000}}
                for i, s in enumerate(steps)]
    json.dump({"battery": "a10", "seam": "size", "provider": "claude", "label": "test", "n": 1, "conversation": CONV,
               "threshold": 200_000, "smoke": False, "account": "test@example.invalid", "claude_version": "0 (test)",
               "build": {"brigd_sha256": "0" * 64, "src_commit": "0" * 40}, "battery_sha256": "1" * 64, "started_ms": sent[0],
               "t0_ms": sent[0], "steps": steps, "requests": requests, "sent_ms": sent, "ended_ms": ended, "contexts": contexts,
               "seams": [], "timed_out": False, "ended_run_ms": ended[-1] + 60_000, "restored_ms": sent[-1] - 1_000},
              open(os.path.join(dirpath, "start.json"), "w"))
    answers = {"p1": p1 or GOOD, "p2": p2 or GOOD}
    seq, events = 0, []
    for s, rid, t, e in zip(steps, requests, sent, ended):
        seq += 1
        events.append({"seq": seq, "atMs": e, "event": {"type": "requestUpdated", "request": {
            "id": rid, "conversationId": CONV, "state": {"type": "done"}, "endedAtMs": e}}})
        text = None
        if s in answers:
            text = "\n".join(f"{n}. {a}" for n, a in sorted(answers[s].items()))
        elif s in ASKS:
            text = (asks or {}).get(s, ASKS[s])
        if text:
            seq += 1
            events.append({"seq": seq, "atMs": t + 40_000, "event": {"type": "messageAppended", "message": {
                "id": f"m-{s}", "conversationId": CONV, "role": "assistant", "requestId": rid, "text": text, "seq": seq}}})
    with open(os.path.join(dirpath, "rec", "events.jsonl"), "w") as f:
        for e in events:
            f.write(json.dumps(e) + "\n")
    at = dict(zip(steps, ended))
    log = []
    for g, step in enumerate(reborn_after, start=1):
        swapped = at[step] + 20_000
        text = ("[Brigadier briefing: only you see this]\n\n[Decisions]\n- (worker claim, unverified) "
                + b["harness"]["plant_learned"]["text"]
                + "\n\n[The conversation's latest messages, verbatim]\nUser: read the two files\n\n[Older context] search\n\n[End of briefing]")
        open(os.path.join(dirpath, "briefings", f"{g:02d}-briefing.txt"), "w").write(text)
        log.append({"streamSeq": g, "atMs": swapped, "entry": {"type": "rebirth", "record": {
            "generation": g, "trigger": "threshold", "atTokens": 210_000, "windowTokens": 1_000_000,
            "prepareStartedAtMs": at[step] + 1_000, "handoffReadyAtMs": at[step] + 15_000, "swapStartedAtMs": swapped - 2_000,
            "swappedAtMs": swapped, "briefingTokens": len(text) // 4, "sections": [], "decisions": 1, "recentMessages": 2}}})
    with open(os.path.join(dirpath, "orchestrator.jsonl"), "w") as f:
        for e in log:
            f.write(json.dumps(e) + "\n")
    json.dump(checks or {s: {"rc": 0, "result": {"constraint": True, "exception": True, "ruled_out": True}}
                         for s in ("e1", "e2", "k3", "k4", "l3", "l4")},
              open(os.path.join(dirpath, "checks.json"), "w"))
    json.dump([], open(os.path.join(dirpath, "manifest.json"), "w"))
    db = sqlite3.connect(os.path.join(dirpath, "data", "routing.sqlite"))
    db.execute("CREATE TABLE turn_usage (at_ms INTEGER, provider TEXT, model TEXT, conversation_id TEXT, task_id TEXT, "
               "input INTEGER, cached_input INTEGER, cache_write INTEGER, output INTEGER, step TEXT)")
    rows = [(t + 30_000, "thread", 10, 1_000, 100, 50) for t in sent]               # one thread turn per step
    for e in log:                                                                    # each rebirth: a note fork
        r = e["entry"]["record"]
        rows.append((r["prepareStartedAtMs"] + 5_000, "handoff", 5, 2_000, 300, 400))
    rows.append((sent[0] - 1, "thread", 999, 999, 999, 999))                         # the warm-up's, before t0
    db.executemany("INSERT INTO turn_usage VALUES (?, 'claude', 'opus', ?, NULL, ?, ?, ?, ?, ?)",
                   [(t, CONV, i, c, w, o, step) for t, step, i, c, w, o in rows])
    db.commit(); db.close()
    return len(steps), len(log)


def all_pass(s):
    return s["batteries"]["thread"]["all_pass"] and s["batteries"]["lookup"]["all_pass"]


CASES = [
    ("all pass, three rebirths", {},
     lambda s, n, r: all_pass(s) and s["valid"] and s["completed"] and s["rebirth_count"] == 3
     and s["quality"]["early"] == {**s["quality"]["early"], "passed": 4, "of": 4}
     and s["quality"]["late"]["passed"] == 4
     and s["usage"]["session"]["turns"] == n + r and s["usage"]["session"]["raw"] == n * 1_160 + r * 2_705
     and s["rebirths"][0]["after_step"] == "b03" and s["rebirths"][0]["raw"] == 2_705 + 100
     and s["rebirths"][0]["prepare_to_swap_s"] == 19.0),
    ("the JSDoc edit after one rebirth only: not tested", dict(reborn_after=("b20",)),
     lambda s, n, r: s["batteries"]["thread"]["by_item"]["constraint"] == "not_tested"
     and s["batteries"]["lookup"]["by_item"]["exception"] == "not_tested" and not s["valid"]),
    ("memory lost the hash, lookup found it: scored apart", dict(p1={**GOOD, 3: "unknown"}),
     lambda s, n, r: s["batteries"]["thread"]["by_item"]["hash"] == "unknown"
     and s["batteries"]["lookup"]["by_item"]["hash"] == "pass"
     and s["batteries"]["either_probe"]["by_item"]["hash"] == "pass" and not s["batteries"]["thread"]["all_pass"]),
    ("lookup guessed what memory knew: lookup fails", dict(p2={**GOOD, 5: "about 3 seconds"}),
     lambda s, n, r: s["batteries"]["lookup"]["by_item"]["units"] == "fail" and s["batteries"]["thread"]["all_pass"]),
    ("a late answer cites the wrong line", dict(asks={"l2": "src/invoice.ts:98: 24 characters."}),
     lambda s, n, r: s["quality"]["late"]["passed"] == 3 and s["quality"]["early"]["passed"] == 4
     and not s["asks"]["l2"]["passed"]),
    ("a late edit's check fails", dict(checks={**{k: {"rc": 0, "result": {"constraint": True, "exception": True, "ruled_out": True}}
                                                  for k in ("e1", "e2", "k3", "k4", "l3")}, "l4": {"rc": 1, "output": "e4: not trimmed"}}),
     lambda s, n, r: s["quality"]["late"]["passed"] == 3 and not s["asks"]["l4"]["passed"]),
]


def main():
    failed = 0
    tmp = tempfile.mkdtemp(prefix="a10-selftest-")
    try:
        for i, (name, kw, ok) in enumerate(CASES):
            d = os.path.join(tmp, f"case{i}")
            n, r = make(d, **kw)
            first = json.dumps(scorer.score(d), sort_keys=True)
            again = subprocess.run([sys.executable, os.path.join(HERE, "a10score.py"), d], check=True, capture_output=True,
                                   text=True).stdout
            same = json.loads(again) == json.loads(first)
            good = ok(json.loads(first), n, r) and same
            failed += not good
            print("ok  " if good else "FAIL", name, "" if same else "(rescoring differs)")
            if not good:
                s = json.loads(first)
                print(json.dumps({k: s[k] for k in ("batteries", "quality", "usage", "rebirths", "valid")}, indent=1)[:4000])
        for root in sys.argv[1:]:
            for run in sorted(os.listdir(os.path.join(root, "runs"))):
                d = os.path.join(root, "runs", run)
                if run.startswith("a10-") and os.path.exists(os.path.join(d, "score.json")):
                    same = scorer.score(d) == json.load(open(os.path.join(d, "score.json")))
                    failed += not same
                    print("ok  " if same else "FAIL", "rescore", run)
    finally:
        shutil.rmtree(tmp)
    print("selftest:", "passed" if not failed else f"{failed} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

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


def make(dirpath, *, p1=None, p2=None, reborn_after=("b03", "b08", "b15"), asks=None, checks=None, battery="a10",
         context_in=None):
    b = load_battery(battery)
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
    for g, (step, used) in enumerate(sorted((context_in or {}).items()), start=100):
        log.append({"streamSeq": g, "atMs": dict(zip(steps, sent))[step] + 5_000, "entry": {"type": "provider", "event": {
            "type": "contextSize", "usedTokens": used, "windowTokens": 1_000_000}}})
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
    reborn = [e for e in log if e["entry"]["type"] == "rebirth"]
    for e in reborn:                                                                 # each rebirth: a note fork
        r = e["entry"]["record"]
        rows.append((r["prepareStartedAtMs"] + 5_000, "handoff", 5, 2_000, 300, 400))
    rows.append((sent[0] - 1, "thread", 999, 999, 999, 999))                         # the warm-up's, before t0
    db.executemany("INSERT INTO turn_usage VALUES (?, 'claude', 'opus', ?, NULL, ?, ?, ?, ?, ?)",
                   [(t, CONV, i, c, w, o, step) for t, step, i, c, w, o in rows])
    db.commit(); db.close()
    return len(steps), len(reborn)


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
    ("an edit before its rebirths is not lost either (review 4)", dict(reborn_after=("b20",), p1={**GOOD, 1: "unknown"}),
     lambda s, n, r: s["batteries"]["thread"]["by_item"]["constraint"] == "not_tested"
     and s["batteries"]["thread"]["unknown_instead_of_guess"]["lost"] == 0),
    ("an ask's context is its first call's, after a rebirth at its start (review 1)", dict(context_in={"l1": 36_000}),
     lambda s, n, r: s["asks"]["l1"]["context_at_send"] == 36_000),
    ("a late edit's check fails", dict(checks={**{k: {"rc": 0, "result": {"constraint": True, "exception": True, "ruled_out": True}}
                                                  for k in ("e1", "e2", "k3", "k4", "l3")}, "l4": {"rc": 1, "output": "e4: not trimmed"}}),
     lambda s, n, r: s["quality"]["late"]["passed"] == 3 and not s["asks"]["l4"]["passed"]),
]


def make_plain(dirpath, *, compact_after=("b12", "b24"), p1=None, asks=None, checks=None):
    """A plain arm's run dir as plain.py leaves one: start.json with each step's record."""
    b = load_battery("a10long")
    os.makedirs(dirpath, exist_ok=True)
    shutil.copyfile(b["path"], os.path.join(dirpath, "battery.md"))
    steps = [s["id"] for s in b["steps"] if s["role"] != "lookup"]
    sent = [1_000_000 + i * STEP_MS for i in range(len(steps))]
    ended = [t + 50_000 for t in sent]
    answers = {"p1": "\n".join(f"{n}. {a}" for n, a in sorted((p1 or GOOD).items())), **ASKS, **(asks or {})}
    records = {}
    for i, s in enumerate(steps):
        records[s] = {"answer": answers.get(s, "ok"), "subtype": "success", "rc": 0, "model_calls": 2, "context": 30_000 + 1_000 * i,
                      "compactions": [{"trigger": "auto", "pre_tokens": 967_000}] if s in compact_after else [],
                      "usage_cumulative": {"input": 10 * (i + 1), "cache_read": 1_000 * (i + 1), "cache_write": 100 * (i + 1),
                                           "output": 50 * (i + 1), "raw": 1_160 * (i + 1)}}
    json.dump({"arm": "plain", "battery": "a10long", "label": "test", "n": 1, "account": "test@example.invalid",
               "claude_version": "0 (test)", "battery_sha256": "1" * 64, "started_ms": sent[0], "t0_ms": sent[0],
               "steps": steps, "sent_ms": sent, "ended_ms": ended, "records": records},
              open(os.path.join(dirpath, "start.json"), "w"))
    json.dump(checks or {s: {"rc": 0, "result": {"constraint": True, "exception": True, "ruled_out": True}}
                         for s in ("e1", "e2", "k3", "k4", "l3", "l4")},
              open(os.path.join(dirpath, "checks.json"), "w"))
    return len(steps), 2


PLAIN_CASES = [
    ("plain: all pass after two compactions", {},
     lambda s, n, r: s["valid"] and s["completed"] and s["compaction_count"] == 2
     and all(v == "pass" for k, v in s["battery"]["by_item"].items() if k != "false_learned")
     and s["battery"]["by_item"]["false_learned"] == "not_applicable"
     and s["quality"]["late"]["passed"] == 4 and s["usage"]["session"]["raw"] == n * 1_160),
    ("plain: one compaction before e1 only: e1's items not tested, invalid", dict(compact_after=("b24",)),
     lambda s, n, r: not s["valid"] and s["battery"]["by_item"]["constraint"] == "not_tested"
     and s["battery"]["by_item"]["hash"] == "pass"),
    ("plain: no compaction: nothing tested", dict(compact_after=()),
     lambda s, n, r: s["battery"]["by_item"]["hash"] == "not_tested" and not s["valid"]),
    ("plain: a guessed number and a broken edit that claims done",
     dict(p1={**GOOD, 5: "about 3 seconds"}, checks={**{k: {"rc": 0, "result": {"constraint": True, "exception": True, "ruled_out": True}}
                                                       for k in ("e1", "e2", "k3", "k4", "l3")}, "l4": {"rc": 1, "output": "e4: not trimmed"}},
          asks={"l4": "Done: parseProduct now trims names, and the checks pass."}),
     lambda s, n, r: s["battery"]["by_item"]["units"] == "fail" and s["battery"]["unknown_instead_of_guess"]["guessed"] == 1
     and s["edit_answers"]["l4"]["unbacked"] and not s["edit_answers"]["l3"]["unbacked"]),
]


def h2h_check(tmp):
    """Review 2: the lookup probe's time isn't Brigadier's in the comparison (every step 50 s: a tie)."""
    h2h = scorer.load("h2h", os.path.join(HERE, "h2h.py"))
    plain = scorer.load("plain", os.path.join(HERE, "plain.py"))
    make(os.path.join(tmp, "h2h-brig"), battery="a10long")
    make_plain(os.path.join(tmp, "h2h-plain"))
    rows = {r[0]: r for r in h2h.rows(scorer.score(os.path.join(tmp, "h2h-brig")), plain.score_run(os.path.join(tmp, "h2h-plain")))}
    work = next(v for k, v in rows.items() if k.startswith("Working time"))
    return work[1] == work[2]


def stream_check():
    """Review 3: plain's model calls count its sub-agents' calls; its context is its own."""
    plain = scorer.load("plain", os.path.join(HERE, "plain.py"))
    u = lambda n: {"input_tokens": 1, "cache_read_input_tokens": n, "cache_creation_input_tokens": 0, "output_tokens": 1}
    rec = plain.step_record([
        {"type": "assistant", "message": {"id": "m1", "usage": u(1_000)}},
        {"type": "assistant", "parent_tool_use_id": "t1", "message": {"id": "s1", "usage": u(50)}},
        {"type": "assistant", "message": {"id": "m2", "usage": u(2_000)}},
        {"type": "result", "result": "done", "subtype": "success", "modelUsage": {}}])
    return rec["model_calls"] == 3 and rec["first_context"] == 1_001 and rec["context"] == 2_001


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
        plain = scorer.load("plain", os.path.join(HERE, "plain.py"))
        for i, (name, kw, ok) in enumerate(PLAIN_CASES):
            d = os.path.join(tmp, f"plain{i}")
            n, r = make_plain(d, **kw)
            first = json.dumps(plain.score_run(d), sort_keys=True)
            again = subprocess.run([sys.executable, os.path.join(HERE, "plain.py"), "score", d], check=True, capture_output=True,
                                   text=True).stdout
            same = json.loads(again) == json.loads(first)
            good = ok(json.loads(first), n, r) and same
            failed += not good
            print("ok  " if good else "FAIL", name, "" if same else "(rescoring differs)")
            if not good:
                s = json.loads(first)
                print(json.dumps({k: s[k] for k in ("battery", "quality", "usage", "valid", "edit_answers")}, indent=1)[:4000])
        for name, check in (("h2h: working time without the lookup probe (review 2)", lambda: h2h_check(tmp)),
                            ("plain: sub-agent calls counted, context its own (review 3)", stream_check)):
            good = check()
            failed += not good
            print("ok  " if good else "FAIL", name)
        for root in sys.argv[1:]:
            for run in sorted(os.listdir(os.path.join(root, "runs"))):
                d = os.path.join(root, "runs", run)
                if not os.path.exists(os.path.join(d, "score.json")):
                    continue
                if run.startswith(("a10-", "a10long-")):
                    same = scorer.score(d) == json.load(open(os.path.join(d, "score.json")))
                elif run.startswith("plain-"):
                    same = plain.score_run(d) == json.load(open(os.path.join(d, "score.json")))
                else:
                    continue
                failed += not same
                print("ok  " if same else "FAIL", "rescore", run)
    finally:
        shutil.rmtree(tmp)
    print("selftest:", "passed" if not failed else f"{failed} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

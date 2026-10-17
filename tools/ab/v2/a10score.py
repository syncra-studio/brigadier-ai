#!/usr/bin/env python3
"""Scores one A10 run (`a10.py`, `battery/a10.md`) from its directory alone.

- **Batteries:** V2's item verdicts (`score.py`), once for `p1` (the thread battery, memory only)
  and once for `p2` (the lookup battery, sent from the copy made before `p1`), each on its own. An
  item an edit acts on (`e1`: the JSDoc rule and its exception; `e2`: the ruled-out approach) is
  `not_tested` unless at least `acted_after_seams` rebirths came between its source and that edit
  and its source had left the last briefing's verbatim tail. "Either probe" is an extra column only.
- **Quality:** the early (`early`) and late (`late`) probes' V3 asks, by V3's own rules: a question
  passes when it cites the expected file:line and every fact; an edit when its hidden check passes on
  the workspace as the run left it. Per ask: the thread's context when it was sent, its time and model
  calls.
- **Rebirths:** count, after which step, the context each was prepared at, the time from prepare to
  swap and the swap's own wait, briefing tokens; and their usage (the note fork's turns, `handoff`, and
  the new session's first turn, which writes the briefing to the cache).
- **Usage (V1's accounting):** the conversation's `turn_usage` from t0 to settlement (raw, cache reads
  and writes apart), per hour of session, without `p1` (kept apart in `p1/`) and the warm-up.
usage: a10score.py <run-dir>   prints the score as JSON"""
import importlib.util, json, os, sqlite3, sys

HERE = os.path.dirname(os.path.abspath(__file__))
V3 = os.path.join(os.path.dirname(HERE), "v3")
sys.path.insert(0, V3)
sys.path.insert(0, HERE)
import v3lib  # noqa: E402
from v2lib import load_battery  # noqa: E402


def load(name, path):
    """A module by its path: V2 and V3 both have a `score.py`."""
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


v2score = load("v2score", os.path.join(HERE, "score.py"))
v3score = load("v3score", os.path.join(V3, "score.py"))
FIELDS = ("input", "cached_input", "cache_write", "output")


def usage(db, where, args):
    """Sums of `turn_usage` rows: uncached input, cache reads, cache writes, output, raw, turns."""
    if db is None:
        return None
    row = db.execute(f"SELECT COUNT(*), {', '.join(f'COALESCE(SUM({f}), 0)' for f in FIELDS)} FROM turn_usage WHERE {where}",
                     args).fetchone()
    out = dict(zip(("turns",) + FIELDS, row))
    out["raw"] = sum(out[f] for f in FIELDS)
    return out


def open_db(path):
    if not os.path.exists(path):
        return None
    db = sqlite3.connect(f"file:{path}?mode=ro&immutable=1", uri=True)
    return db


def after_seams(s, b, start, briefs, need):
    """Marks `not_tested` an item whose edit came before `need` rebirths since its source, or while
    its source was still in the verbatim tail."""
    sent = dict(zip(start["steps"], start["sent_ms"]))
    items = {i["id"]: i for i in b["items"]}
    for iid, d in s["items"].items():
        step = items[iid].get("acted")
        if not step or step not in sent:
            continue
        src = sent.get(items[iid]["source"], 0)
        between = [x for x in briefs if src < x["at_ms"] < sent[step]]
        left = d["tail"][0]["left_tail"] if d.get("tail") else None
        d["acted_after_seams"] = {"rebirths": len(between), "need": need, "left_tail": left}
        if len(between) < need or left is not True:
            s["by_item"][iid] = "not_tested"
    s["all_pass"] = s["completed"] and s["valid"] and all(v == "pass" for v in s["by_item"].values())
    return s


def score(run):
    start = json.load(open(os.path.join(run, "start.json")))
    b = load_battery(os.path.join(run, "battery.md"))
    h = b["harness"]
    rec = v3lib.Record(run)
    log = v2score.orchestrator(run)
    briefs = v2score.briefings(run, log)
    checks = json.load(open(os.path.join(run, "checks.json"))) if os.path.exists(os.path.join(run, "checks.json")) else {}
    sent = dict(zip(start["steps"], start["sent_ms"]))
    ended = dict(zip(start["steps"], start.get("ended_ms") or []))
    rid = dict(zip(start["steps"], start["requests"]))
    contexts = {c["step"]: c for c in start.get("contexts", [])}
    order = start["steps"]
    roles = {s["id"]: s["role"] for s in b["steps"]}

    batteries = {}
    for probe, name in (h.get("probes") or {}).items():
        if probe not in sent:
            batteries[name] = {"probe": probe, "sent": False}
            continue
        s = after_seams(v2score.score(run, probe), b, start, briefs, h.get("acted_after_seams", 0))
        batteries[name] = {"probe": probe, "sent": True, "completed": s["completed"], "valid": s["valid"],
                           "seams": s["seams"], "by_item": s["by_item"], "all_pass": s["all_pass"],
                           "unknown_instead_of_guess": s["unknown_instead_of_guess"], "probe_tool_calls": s["probe_tool_calls"],
                           "why": "; ".join(f"{i}: {v}" for i, v in s["by_item"].items() if v != "pass") or "all items pass",
                           "items": s["items"]}
    if len([x for x in batteries.values() if x.get("sent")]) == 2:
        a, c = [x["by_item"] for x in batteries.values()]
        batteries["either_probe"] = {"extra_column_only": True,
                                     "by_item": {i: "pass" if "pass" in (a[i], c.get(i)) else a[i] for i in a}}

    asks = {}
    for step, ask_id in (h.get("asks") or {}).items():
        if step not in sent:
            continue
        ask = v3lib.ask_files([ask_id])[0]
        lo, hi = sent[step], ended.get(step) or sent[step]
        prev = order[order.index(step) - 1] if order.index(step) else None
        d = {"ask": ask_id, "probe": roles[step], "kind": ask["kind"], "context_at_send": (contexts.get(prev) or {}).get("used"),
             "seconds": round((hi - lo) / 1000, 1), "model_calls": v3score.model_calls(run, rec, lo, hi)}
        if ask["rule"].get("rule") == "answer":
            ok, why, info = v3score.score_answer(ask["rule"], rec.answer(rid[step]))
            d.update(passed=ok, why=why, cited=info["cited"][:6])
        else:
            c = checks.get(step) or {}
            d.update(passed=c.get("rc") == 0, why="hidden check passed" if c.get("rc") == 0 else (c.get("output") or "")[-300:])
        asks[step] = d
    quality = {}
    for probe in ("early", "late"):
        mine = {k: v for k, v in asks.items() if v["probe"] == probe}
        if mine:
            ctx = [v["context_at_send"] for v in mine.values() if v["context_at_send"]]
            quality[probe] = {"passed": sum(v["passed"] for v in mine.values()), "of": len(mine),
                              "context_range": [min(ctx), max(ctx)] if ctx else None,
                              "seconds": round(sum(v["seconds"] for v in mine.values()), 1),
                              "model_calls": sum(v["model_calls"]["total"] for v in mine.values())}

    db = open_db(os.path.join(run, "data", "routing.sqlite"))
    conv = start["conversation"]
    times = json.load(open(os.path.join(run, "times.json"))) if os.path.exists(os.path.join(run, "times.json")) else {}
    t0, settled = start["t0_ms"], times.get("settled_ms") or start.get("ended_run_ms") or start["t0_ms"]
    total = usage(db, "conversation_id = ? AND at_ms BETWEEN ? AND ?", (conv, t0, settled))
    hours = (settled - t0) / 3_600_000
    p1 = None
    if "p1" in sent:
        p1db = open_db(os.path.join(run, "p1", "routing.sqlite"))
        p1 = usage(p1db, "conversation_id = ? AND at_ms BETWEEN ? AND ?", (conv, sent["p1"], start.get("restored_ms") or settled))

    # The cross-check: every assistant message of the run's Claude transcripts once (a rebirth's
    # note fork copies the session's earlier messages, which tokens.py counts again).
    seen = {}
    for dirpath, _, names in os.walk(os.path.join(run, "transcripts", "claude")):
        for name in sorted(names):
            for line in open(os.path.join(dirpath, name), errors="replace"):
                try:
                    r = json.loads(line)
                except ValueError:
                    continue
                m = r.get("message") or {}
                if r.get("type") == "assistant" and m.get("id") and m.get("usage") and r.get("timestamp") \
                        and t0 <= v3lib.tsms(r["timestamp"]) <= settled:
                    u = m["usage"]
                    seen[m["id"]] = sum(u.get(k) or 0 for k in ("input_tokens", "cache_read_input_tokens",
                                                               "cache_creation_input_tokens", "output_tokens"))
    reborn = []
    for x in briefs:
        r = next(e["entry"]["record"] for e in log if e["entry"].get("type") == "rebirth"
                 and e["entry"]["record"]["generation"] == x["generation"])
        # Prepared after the turn that crossed the threshold (before the next step is sent), swapped
        # at the start of the next one.
        after = max((s for s in order if sent[s] < r["prepareStartedAtMs"]), key=lambda s: sent[s], default=None)
        nxt = db.execute("SELECT MIN(at_ms) FROM turn_usage WHERE conversation_id = ? AND step = 'thread' AND at_ms > ?",
                         (conv, r["swappedAtMs"])).fetchone()[0] if db else None
        first = usage(db, "conversation_id = ? AND step = 'thread' AND at_ms = ?", (conv, nxt)) if nxt else None
        note = usage(db, "conversation_id = ? AND step = 'handoff' AND at_ms BETWEEN ? AND ?",
                     (conv, r["prepareStartedAtMs"], r["swappedAtMs"]))
        reborn.append({"generation": r["generation"], "trigger": r.get("trigger"), "after_step": after,
                       "at_tokens": r.get("atTokens"), "window": r.get("windowTokens"),
                       "prepare_to_swap_s": round((r["swappedAtMs"] - r["prepareStartedAtMs"]) / 1000, 1),
                       "swap_wait_s": round((r["swappedAtMs"] - r["swapStartedAtMs"]) / 1000, 1) if r.get("swapStartedAtMs") else None,
                       "note_ready_s": round((r["handoffReadyAtMs"] - r["prepareStartedAtMs"]) / 1000, 1) if r.get("handoffReadyAtMs") else None,
                       "briefing_tokens": r.get("briefingTokens"), "note_usage": note, "first_turn_usage": first,
                       "raw": (note or {}).get("raw", 0) + (first or {}).get("cache_write", 0)})
    windows = sorted({c["window"] for c in start.get("contexts", []) if c.get("window")})
    peak = max((c["used"] or 0 for c in start.get("contexts", [])), default=0)
    completed = not start.get("timed_out") and all((rec.requests.get(r) or {}).get("state", {}).get("type") in ("done", "waiting")
                                                   for r in start["requests"])
    return {
        "threshold": start["threshold"], "label": start["label"], "n": start["n"], "smoke": start.get("smoke"),
        "account": start.get("account"), "claude_version": start.get("claude_version"),
        "brigd_sha256": start["build"]["brigd_sha256"], "src_commit": start["build"]["src_commit"],
        "battery_sha256": start.get("battery_sha256"), "started_ms": start.get("started_ms"),
        "completed": completed, "valid": all(x.get("valid") for x in batteries.values() if "valid" in x),
        "batteries": {k: {kk: vv for kk, vv in v.items() if kk != "items"} for k, v in batteries.items()},
        "quality": quality, "asks": asks,
        "rebirth_count": len(reborn), "rebirths": reborn, "windows": windows, "peak_context": peak,
        "contexts": [[c["step"], c["used"]] for c in start.get("contexts", [])],
        "usage": {"session": total, "hours": round(hours, 3), "raw_per_hour": round(total["raw"] / hours) if total and hours else None,
                  "rebirths_raw": sum(x["raw"] for x in reborn), "p1_apart": p1,
                  "transcripts_raw_once": sum(seen.values()), "transcripts_messages": len(seen)},
        "edits": {k: {"rc": v.get("rc"), "result": v.get("result")} for k, v in checks.items()},
        "item_details": {k: v["items"] for k, v in batteries.items() if "items" in v},
    }


if __name__ == "__main__":
    print(json.dumps(score(sys.argv[1]), indent=1, sort_keys=True))

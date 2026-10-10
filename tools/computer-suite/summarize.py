#!/usr/bin/env python3
"""Tables from a model run's trials (run.py) and the scripted run's reference batches.
usage: summarize.py <reference scripted.json> <run dir> [<run dir> ...] [--json out]

Per trial: pass, wrong-target events, rungs, batches, model calls, E1 (calls ÷ the scripted
solver's tool calls + 2; the gate is a median ≤ 1.3, as the user ruled on 2026-10-09), calls per
reference batch (the gate before that ruling, kept for comparison), wall time, the model the
attempts actually ran on, and the usage of the worker and of the thread's relay turns.
With several runs, a closing table per provider pools their trials.

F1, from each run's focus monitor (`suite watch`, from the first setup to the last teardown): a
change is the suite's only with a cause of the suite's just before it (see `focus`). The user may
be at the Mac; every other change is listed as the user's or another program's, and counted apart."""
import glob, json, os, sys

TOKENS = ("input_uncached", "cache_read", "cache_write", "output")
E1_GATE = 1.3


def load(run):
    out = []
    for f in sorted(glob.glob(os.path.join(run, "*", "result.json"))):
        out.append(json.load(open(f)))
    return out


def tokens(rows):
    t = {k: sum(r.get(k) or 0 for r in rows) for k in TOKENS}
    t["turns"] = sum(r.get("turns") or 0 for r in rows)
    return t


def focus(run, results):
    """Splits the focus monitor's changes into the suite's and everyone else's. The user may be at
    the Mac, so a change is the suite's only with a cause of the suite's just before it:
    - the frontmost app became a trial's target within 2 s after one of the trials' actions, or
      while its fixture was being opened (up to 2 s after the target was ready);
    - the real cursor moved once within 300 ms after an action, still for 1 s before it and after
      (a person's moves come in a stream, 50 ms apart; the engine never moves the cursor at all).
    A target that came to the front with no such cause (a click on its Dock icon) is listed apart."""
    pids = {r["pid"] for r in results if "pid" in r}
    acts = []
    launches = []
    for r in results:
        d = os.path.join(run, r["task"].replace(" ", "-"))
        if os.path.exists(os.path.join(d, "actions.json")):
            acts += [a["atMs"] for a in json.load(open(os.path.join(d, "actions.json")))]
        setup = os.path.join(d, "target", "setup.json")
        if os.path.exists(setup) and r.get("t0_ms"):
            s = json.load(open(setup))
            launches.append((s.get("pid"), r["t0_ms"] - 30000, s.get("ready_ms", 0) + 2000))
    events = []
    for f in glob.glob(os.path.join(run, "focus-*.jsonl")):
        events += [e for e in map(json.loads, open(f)) if "changed" in e]
    events.sort(key=lambda e: e["at_ms"])
    moves = [e["at_ms"] for e in events if "cursor" in e["changed"]]
    ours, unexplained, others = [], [], []
    for e in events:
        to, t = e["to"], e["at_ms"]
        why, front_target = [], False
        if "frontmost_app" in e["changed"] and to.get("frontmost_pid") in pids:
            front_target = True
            if any(0 <= t - a <= 2000 for a in acts):
                why.append(f"frontmost became target pid {to['frontmost_pid']} after an action")
            if any(p == to.get("frontmost_pid") and lo <= t <= hi for p, lo, hi in launches):
                why.append(f"frontmost became target pid {to['frontmost_pid']} as it opened")
        if "cursor" in e["changed"]:
            for a in acts:
                if (0 <= t - a <= 300 and not any(a - 1000 <= m < a for m in moves)
                        and not any(t < m <= t + 1000 for m in moves)):
                    why.append("cursor moved right after an action, still before it")
                    break
        row = {"at_ms": t, "changed": e["changed"], "why": why, "to_pid": to.get("frontmost_pid")}
        if why:
            ours.append(row)
        elif front_target:
            unexplained.append(row)
        else:
            others.append(row)
    return {"suite": ours, "target_front_without_cause": unexplained, "other": len(others),
            "other_changes": others[:20]}


def median(xs):
    xs = sorted(x for x in xs if x is not None)
    return xs[len(xs) // 2] if xs else None


def gate(e1):
    """E1's gate: the median model calls per (scripted tool calls + 2) at most 1.3."""
    return "no reference" if e1 is None else ("pass" if e1 <= E1_GATE else "MISS")


def providers(out):
    """Every run of a provider pooled: trials, passes, E1, wall time, usage per completed task."""
    pooled = {}
    for name, r in out["runs"].items():
        pooled.setdefault(name.split("-")[0], []).append((name, r))
    rows = {}
    for prov, runs in pooled.items():
        trials = [x for _, r in runs for x in r["rows"]]
        passed = sum(r["passed"] for _, r in runs)
        per = {k: {f: round(sum(r["usage_total"][k][f] for _, r in runs) / max(1, passed)) for f in TOKENS}
               for k in ("worker", "thread")}
        rows[prov] = {"runs": [n for n, _ in runs], "trials": len(trials), "passed": passed,
                      "e1_median": median([x["e1"] for x in trials]),
                      "per_batch_median": median([x["per_batch"] for x in trials]),
                      "calls_median": median([x["model_calls"] for x in trials]),
                      "wall_s_median": median([x["wall_s"] for x in trials]),
                      "f1_suite_changes": sum(len(r["focus"]["suite"]) for _, r in runs),
                      "usage_per_completed_task": per}
    return rows


def main():
    scripted = json.load(open(sys.argv[1]))
    ref = {r["task"]: r["reference_batches"] for r in scripted}
    # E1's reference: the scripted solver's tool calls, plus a look before acting and a report.
    ref2 = {r["task"]: r["tool_calls"] + 2 for r in scripted if r.get("tool_calls")}
    runs = [a for a in sys.argv[2:] if not a.startswith("--") and not a.endswith(".json")]
    out = {"runs": {}}
    for run in runs:
        results = load(run)
        rows, totals = [], {"worker": [], "thread": []}
        for r in results:
            v = r.get("verdict") or {}
            calls = (r.get("calls") or {}).get("model_calls")
            refb = ref.get(r["task"])
            u = r.get("usage") or {}
            totals["worker"] += u.get("worker", []); totals["thread"] += u.get("thread", [])
            rows.append({
                "task": r["task"], "pass": bool(v.get("pass")), "outcome": v.get("outcome"),
                "wrong_target": v.get("wrong_target"), "rungs": v.get("rungs"), "batches": r.get("batches"),
                "reference_batches": refb, "model_calls": calls,
                "e1": round(calls / ref2[r["task"]], 2) if calls and r["task"] in ref2 else None,
                "per_batch": round(calls / refb, 2) if calls and refb else None,
                "child_calls": (r.get("calls") or {}).get("child_calls"),
                "wall_s": round((r.get("wall_ms") or 0) / 1000, 1),
                "worker_s": round((r.get("worker_ms") or 0) / 1000, 1),
                "models": sorted({f"{a['provider']}:{a['model']}:{a['effort']}" for a in r.get("attempts", [])}),
                "fallbacks": max(0, len(r.get("attempts", [])) - 1),
                "worker_tokens": tokens(u.get("worker", [])), "thread_tokens": tokens(u.get("thread", [])),
                "shortcuts": (r.get("audit") or {}).get("shortcuts"),
                "other_tools": len((r.get("audit") or {}).get("other_tools") or []),
                "notes": v.get("notes"), "error": r.get("error"),
            })
        passed = [x for x in rows if x["pass"]]
        w, t = tokens(totals["worker"]), tokens(totals["thread"])
        n = max(1, len(passed))
        out["runs"][os.path.basename(run.rstrip("/"))] = {
            "trials": len(rows), "passed": len(passed), "rows": rows,
            "usage_total": {"worker": w, "thread": t},
            "usage_per_completed_task": {k: {f: round(v / n) for f, v in x.items()} for k, x in (("worker", w), ("thread", t))},
            "e1_median": median([x["e1"] for x in rows]),
            "per_batch_median": median([x["per_batch"] for x in rows]),
            "focus": focus(run, results),
        }
    out["providers"] = providers(out)
    js = json.dumps(out, indent=1)
    if "--json" in sys.argv:
        open(sys.argv[sys.argv.index("--json") + 1], "w").write(js)
    for name, r in out["runs"].items():
        print(f"## {name}: {r['passed']}/{r['trials']} passed, E1 median {r['e1_median']} ({gate(r['e1_median'])}; "
              f"per reference batch {r['per_batch_median']}), "
              f"F1 suite changes {len(r['focus']['suite'])} (target to front with no suite cause "
              f"{len(r['focus']['target_front_without_cause'])}, other {r['focus']['other']})")
        print("| task | pass | batches | ref | calls | E1 | per batch | wall s | model | worker in/cache/out | thread in/cache/out | notes |")
        print("|---|---|---|---|---|---|---|---|---|---|---|---|")
        for x in r["rows"]:
            wt, tt = x["worker_tokens"], x["thread_tokens"]
            print(f"| {x['task']} | {'pass' if x['pass'] else 'FAIL'}{' ' + x['outcome'] if x['outcome'] else ''} | {x['batches']} | "
                  f"{x['reference_batches']} | {x['model_calls']} | {x['e1']} | {x['per_batch']} | {x['wall_s']} | {', '.join(x['models'])} | "
                  f"{wt['input_uncached']}/{wt['cache_read']}/{wt['output']} | {tt['input_uncached']}/{tt['cache_read']}/{tt['output']} | "
                  f"{'; '.join(x['notes'] or [])[:160]}{x['error'] or ''} |")
        print("per completed task:", json.dumps(r["usage_per_completed_task"]))
    if len(out["runs"]) > 1:
        print("\n## Per provider, runs pooled")
        print("| provider | runs | passed | median calls | E1 median | E1 gate (≤ 1.3) | per batch median | median wall s | F1 suite changes | worker in/cache-read/cache-write/out per completed task | thread, the same |")
        print("|---|---|---|---|---|---|---|---|---|---|---|")
        for prov, p in out["providers"].items():
            w, t = p["usage_per_completed_task"]["worker"], p["usage_per_completed_task"]["thread"]
            print(f"| {prov} | {len(p['runs'])} | {p['passed']}/{p['trials']} | {p['calls_median']} | {p['e1_median']} | "
                  f"{gate(p['e1_median'])} | {p['per_batch_median']} | {p['wall_s_median']} | {p['f1_suite_changes']} | "
                  f"{'/'.join(str(w[k]) for k in TOKENS)} | {'/'.join(str(t[k]) for k in TOKENS)} |")


if __name__ == "__main__":
    main()

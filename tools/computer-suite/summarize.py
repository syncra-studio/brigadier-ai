#!/usr/bin/env python3
"""Tables from a model run's trials (run.py) and the scripted run's reference batches.
usage: summarize.py <reference scripted.json> <run dir> [<run dir> ...] [--json out]

Per trial: pass, wrong-target events, rungs, batches, model calls, E1 (calls ÷ the fewest `act`
batches the scripted solver needed), wall time, the model the attempts actually ran on, and the
usage of the worker and of the thread's relay turns.

F1, from each run's focus monitor (`suite watch`, from the first setup to the last teardown): a
change is the suite's when the user's frontmost app became one of the trials' targets, or when the
real cursor moved within 300 ms of one of the trials' actions. Every other change is listed as
the user's or another program's, and counted apart."""
import glob, json, os, sys

TOKENS = ("input_uncached", "cache_read", "cache_write", "output")


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
    pids = {r["pid"] for r in results if "pid" in r}
    acts = []
    for r in results:
        d = os.path.join(run, r["task"].replace(" ", "-"), "actions.json")
        if os.path.exists(d):
            acts += [a["atMs"] for a in json.load(open(d))]
    ours, others = [], []
    for f in glob.glob(os.path.join(run, "focus-*.jsonl")):
        for line in open(f):
            e = json.loads(line)
            if "changed" not in e:
                continue
            frm, to = e["from"], e["to"]
            why = []
            if "frontmost_app" in e["changed"] and to.get("frontmost_pid") in pids:
                why.append(f"frontmost became target pid {to['frontmost_pid']}")
            if "cursor" in e["changed"] and any(abs(e["at_ms"] - a) < 300 for a in acts):
                why.append("cursor moved during an action")
            (ours if why else others).append({"at_ms": e["at_ms"], "changed": e["changed"], "why": why,
                                              "to_pid": to.get("frontmost_pid")})
    return {"suite": ours, "other": len(others), "other_changes": others[:20]}


def main():
    ref = {r["task"]: r["reference_batches"] for r in json.load(open(sys.argv[1]))}
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
                "e1": round(calls / refb, 2) if calls and refb else None,
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
            "e1_median": sorted(x["e1"] for x in rows if x["e1"])[len([x for x in rows if x["e1"]]) // 2] if any(x["e1"] for x in rows) else None,
            "focus": focus(run, results),
        }
    js = json.dumps(out, indent=1)
    if "--json" in sys.argv:
        open(sys.argv[sys.argv.index("--json") + 1], "w").write(js)
    for name, r in out["runs"].items():
        print(f"## {name}: {r['passed']}/{r['trials']} passed, E1 median {r['e1_median']}, "
              f"F1 suite changes {len(r['focus']['suite'])} (other {r['focus']['other']})")
        print("| task | pass | batches | ref | calls | E1 | wall s | model | worker in/cache/out | thread in/cache/out | notes |")
        print("|---|---|---|---|---|---|---|---|---|---|---|")
        for x in r["rows"]:
            wt, tt = x["worker_tokens"], x["thread_tokens"]
            print(f"| {x['task']} | {'pass' if x['pass'] else 'FAIL'}{' ' + x['outcome'] if x['outcome'] else ''} | {x['batches']} | "
                  f"{x['reference_batches']} | {x['model_calls']} | {x['e1']} | {x['wall_s']} | {', '.join(x['models'])} | "
                  f"{wt['input_uncached']}/{wt['cache_read']}/{wt['output']} | {tt['input_uncached']}/{tt['cache_read']}/{tt['output']} | "
                  f"{'; '.join(x['notes'] or [])[:160]}{x['error'] or ''} |")
        print("per completed task:", json.dumps(r["usage_per_completed_task"]))


if __name__ == "__main__":
    main()

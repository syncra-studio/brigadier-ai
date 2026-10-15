#!/usr/bin/env python3
"""V3/V1 summary: one table per regime x Brain (cold and warm, seeded and empty, never pooled),
every attempt counted, the completion rate first. Builds (labels) are listed side by side.

Reads every attempt under the given harness roots or run dirs: `runs/*/score.json`, plus the
attempts in `results.jsonl` that never got a score (harness errors), which count as not completed.
Every recorded run is scored again first (score.py, deterministic and cheap), so a scorer change
always applies to old runs; --stored reads the score.json written at run time instead.
usage: summary.py <root-or-run>... [--stored] [--json out.json]"""
import collections, glob, json, os, statistics, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))


def attempts(paths, rescore):
    out, seen = [], set()
    for p in paths:
        runs = [p] if os.path.exists(os.path.join(p, "start.json")) else sorted(glob.glob(os.path.join(p, "runs", "*")))
        for run in runs:
            sp = os.path.join(run, "score.json")
            if rescore and os.path.exists(os.path.join(run, "freeze.json")):
                s = json.loads(subprocess.run([sys.executable, os.path.join(HERE, "score.py"), run], check=True,
                                              capture_output=True, text=True).stdout)
            elif os.path.exists(sp):
                s = json.load(open(sp))
            else:
                continue
            s["run"] = os.path.basename(run); seen.add(s["run"]); out.append(s)
        res = os.path.join(p, "results.jsonl")
        if os.path.exists(res):
            for line in open(res):
                r = json.loads(line)
                if r["run"] not in seen and str(r.get("why", "")).startswith("harness error"):
                    seen.add(r["run"]); out.append(r)
    return out


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def fmt(v):
    if v is None:
        return "–"
    if isinstance(v, float) and not v.is_integer():
        return f"{v:,.1f}"
    return f"{int(v):,}"


def spread(xs):
    xs = [x for x in xs if x is not None]
    if not xs:
        return "–"
    return fmt(med(xs)) if len(xs) == 1 else f"{fmt(med(xs))} ({fmt(min(xs))}–{fmt(max(xs))})"


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    rescore = "--stored" not in sys.argv
    rows = attempts(args, rescore)
    groups = collections.defaultdict(list)
    for r in rows:
        groups[(r.get("regime"), r.get("brain"))].append(r)
    report = {}
    for (regime, brain), rs in sorted(groups.items(), key=lambda kv: (str(kv[0][0]), str(kv[0][1]))):
        print(f"## {regime} · {brain} Brain\n")
        labels = sorted({r.get("label") for r in rs})
        print("| Build | Attempts | Completed | Passed | Model calls (median) | Worker starts | Answer s (median) | Raw tokens (median) | Usage complete |")
        print("|---|---|---|---|---|---|---|---|---|")
        for lab in labels:
            ls = [r for r in rs if r.get("label") == lab]
            done = [r for r in ls if r.get("completed")]
            print(f"| {lab} | {len(ls)} | {len(done)}/{len(ls)} ({100 * len(done) // max(1, len(ls))}%) | "
                  f"{sum(1 for r in ls if r.get('pass'))}/{len(ls)} | "
                  f"{fmt(med([(r.get('model_calls') or {}).get('total') for r in done]))} | "
                  f"{fmt(med([r.get('worker_starts') for r in done]))} | {fmt(med([r.get('answer_s') for r in done]))} | "
                  f"{fmt(med([(r.get('tokens') or {}).get('raw') for r in done]))} | "
                  f"{sum(1 for r in ls if (r.get('tokens') or {}).get('complete'))}/{len(ls)} |")
        print()
        print("| Ask | Build | n | Pass | Calls total (thread) | Worker starts | Answer s | Settled s | Raw tokens | Raw without cache reads | Cache reads | Why (failures) |")
        print("|---|---|---|---|---|---|---|---|---|---|---|---|")
        per = collections.defaultdict(list)
        for r in rs:
            per[(r.get("ask"), r.get("label"))].append(r)
        for (ask, lab), xs in sorted(per.items(), key=lambda kv: (str(kv[0][0]), str(kv[0][1]))):
            xs.sort(key=lambda r: r.get("n") or 0)
            calls = [(r.get("model_calls") or {}).get("total") for r in xs]
            thread = [(r.get("model_calls") or {}).get("thread") for r in xs]
            tok = lambda k: [(r.get("tokens") or {}).get(k) for r in xs]
            fails = sorted({r.get("why") for r in xs if not r.get("pass")})
            print(f"| {ask} | {lab} | {len(xs)} | {''.join('P' if r.get('pass') else 'F' for r in xs)} | "
                  f"{spread(calls)} ({spread(thread)}) | {spread([r.get('worker_starts') for r in xs])} | "
                  f"{spread([r.get('answer_s') for r in xs])} | {spread([r.get('settled_s') for r in xs])} | {spread(tok('raw'))} | {spread(tok('raw_without_cache_reads'))} | "
                  f"{spread(tok('cache_read'))} | {'; '.join(fails)[:160]} |")
        print()
        agree = {f"{ask}@{lab}": len({bool(r.get('pass')) for r in xs}) == 1 for (ask, lab), xs in per.items() if len(xs) >= 2}
        if agree:
            print("Agreement (same pass/fail on every attempt of an ask): " +
                  ", ".join(f"{k} {'agrees' if v else 'DISAGREES'}" for k, v in sorted(agree.items())) + "\n")
        report[f"{regime}/{brain}"] = {"attempts": rs, "agreement": agree}
    if "--json" in sys.argv:
        json.dump(report, open(sys.argv[sys.argv.index("--json") + 1], "w"), indent=1, sort_keys=True)


if __name__ == "__main__":
    main()

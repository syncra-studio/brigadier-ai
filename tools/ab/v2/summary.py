#!/usr/bin/env python3
"""V2 runs side by side, rescored: one table per battery x seam x provider x build, an item per row
and a run per column, and whether the reruns agree on every item (BUILD-PLAN D1.0's "three reruns
on an unchanged build agree per item"). Invalid or incomplete runs are shown and named, never
dropped.

usage: summary.py <harness-root> [<harness-root> ...] [--json]
Exit 0 when every group of two or more runs agrees per item, 1 otherwise."""
import glob, json, os, sys
from collections import defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import score as scorer  # noqa: E402


def main():
    roots = [a for a in sys.argv[1:] if not a.startswith("--")]
    as_json = "--json" in sys.argv
    groups = defaultdict(list)
    for root in roots:
        for run in sorted(glob.glob(os.path.join(root, "runs", "*"))):
            if not os.path.exists(os.path.join(run, "start.json")):
                continue
            s = scorer.score(run)
            groups[(s["battery"], s["seam"], s["provider"], s["label"])].append((os.path.basename(run), s))
    out, disagree = [], 0
    for key, runs in sorted(groups.items()):
        runs.sort(key=lambda r: r[1]["n"])
        items = list(runs[0][1]["by_item"])
        agree = {i: len({s["by_item"].get(i) for _, s in runs}) == 1 for i in items}
        group = {"battery": key[0], "seam": key[1], "provider": key[2], "label": key[3], "runs": [r for r, _ in runs],
                 "valid": [s["valid"] for _, s in runs], "completed": [s["completed"] for _, s in runs],
                 "seams_forced": [s["seams"]["forced"] for _, s in runs],
                 "items": {i: [s["by_item"].get(i) for _, s in runs] for i in items},
                 "agree": agree, "all_agree": all(agree.values()),
                 "unknown_instead_of_guess": [s["unknown_instead_of_guess"]["score"] for _, s in runs]}
        if len(runs) >= 2 and not group["all_agree"]:
            disagree += 1
        out.append(group)
    if as_json:
        print(json.dumps(out, indent=1))
    else:
        for g in out:
            print(f"\n{g['battery']} · seam {g['seam']} · {g['provider']} · {g['label']}  "
                  f"({len(g['runs'])} runs; reruns {'agree' if g['all_agree'] else 'DISAGREE'})")
            print(f"  {'valid':<16}" + "".join(f"{str(v):<12}" for v in g["valid"]))
            print(f"  {'completed':<16}" + "".join(f"{str(v):<12}" for v in g["completed"]))
            print(f"  {'seams forced':<16}" + "".join(f"{v:<12}" for v in g["seams_forced"]))
            for i, statuses in g["items"].items():
                mark = "" if g["agree"][i] else "  <- differs"
                print(f"  {i:<16}" + "".join(f"{str(v):<12}" for v in statuses) + mark)
            print(f"  {'unknown score':<16}" + "".join(f"{str(v):<12}" for v in g["unknown_instead_of_guess"]))
    sys.exit(1 if disagree else 0)


if __name__ == "__main__":
    main()

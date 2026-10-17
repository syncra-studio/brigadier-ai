#!/usr/bin/env python3
"""The long-session head-to-head's table: Brigadier (`a10.py --battery a10long`, scored by
a10score.py) against plain Claude Code (`plain.py`), from their two run directories alone, with a
winner per row. Memory is the thread battery's `p1` for both (the false `learned` line, which only
Brigadier's daemon plants, is left out of the shared count); Brigadier's lookup probe `p2` is an
extra row. Tokens are through settlement, with Brigadier's `p1` put back and its `p2` taken out, so
both arms count the same steps. Time is working time: each step from its send to its end.

usage: h2h.py <brigadier-run-dir> <plain-run-dir> [--json out]   prints the table as Markdown"""
import importlib.util, json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def rows(brig, plain):
    shared = [i for i, v in plain["battery"]["by_item"].items() if v != "not_applicable"]
    bt, pt = brig["batteries"]["thread"]["by_item"], plain["battery"]["by_item"]
    passed = lambda by: sum(1 for i in shared if by.get(i) == "pass")
    tested = lambda by: sum(1 for i in shared if by.get(i) != "not_tested")
    common = [i for i in shared if bt.get(i) != "not_tested" and pt.get(i) != "not_tested"]
    both = lambda by: sum(1 for i in common if by.get(i) == "pass")
    unbacked = lambda s, guessed: guessed + sum(1 for e in s["edit_answers"].values() if e["unbacked"])
    bg = brig["batteries"]["thread"]["unknown_instead_of_guess"]["guessed"]
    pg = plain["battery"]["unknown_instead_of_guess"]["guessed"]
    bu, pu = brig["usage"], plain["usage"]
    btok = bu["session"]["raw"] + ((bu.get("p1_apart") or {}).get("raw") or 0) - ((bu.get("p2_apart") or {}).get("raw") or 0)
    ptok = pu["session"]["raw"]
    edits = lambda s: sum(1 for e in s["edit_answers"].values() if e["check_passed"])
    q = lambda s, k: (s["quality"].get(k) or {}).get("passed", 0)
    out = [
        # (row, brigadier, plain, which is better: "more" / "less" / None, shown values)
        ("Memory after the seams (p1: planted items recalled, of those tested in both arms)", both(bt), both(pt), "more",
         f"{both(bt)}/{len(common)} ({passed(bt)}/{tested(bt)} tested)", f"{both(pt)}/{len(common)} ({passed(pt)}/{tested(pt)} tested)"),
        ("Guesses instead of \"unknown\" (p1)", bg, pg, "less", str(bg), str(pg)),
        ("Late quality probe (V3 q3, q4, e2, e4)", q(brig, "late"), q(plain, "late"), "more",
         f"{q(brig, 'late')}/4", f"{q(plain, 'late')}/4"),
        ("Early quality probe (V3 q1, q2, e1, e3)", q(brig, "early"), q(plain, "early"), "more",
         f"{q(brig, 'early')}/4", f"{q(plain, 'early')}/4"),
        ("Edits whose hidden checks pass", edits(brig), edits(plain), "more",
         f"{edits(brig)}/{len(brig['edit_answers'])}", f"{edits(plain)}/{len(plain['edit_answers'])}"),
        ("Claims it can't back up (guesses + broken edits reported done)", unbacked(brig, bg), unbacked(plain, pg), "less",
         str(unbacked(brig, bg)), str(unbacked(plain, pg))),
        ("Tokens through settlement (raw, all calls)", btok, ptok, "less", f"{btok:,}", f"{ptok:,}"),
        ("Working time (sum of steps)", bu["working_s"], pu["working_s"], "less",
         f"{bu['working_s'] / 60:.0f} min", f"{pu['working_s'] / 60:.0f} min"),
        ("Seams (Brigadier rebirths / plain compactions)", brig["rebirth_count"], plain["compaction_count"], None,
         str(brig["rebirth_count"]), str(plain["compaction_count"])),
        ("Peak context", brig["peak_context"], plain["peak_context"], None, f"{brig['peak_context']:,}", f"{plain['peak_context']:,}"),
    ]
    if brig["batteries"].get("lookup", {}).get("sent"):
        lt = brig["batteries"]["lookup"]["by_item"]
        out.append(("Brigadier only: memory with session search (p2)", passed(lt), None, None,
                    f"{passed(lt)}/{tested(lt)} tested", "no session search"))
    return out


def table(brig, plain):
    lines = ["| | Brigadier | Plain Claude Code | Winner |", "|---|---|---|---|"]
    for name, b, p, better, bs, ps in rows(brig, plain):
        if better is None or p is None:
            win = "—"
        elif b == p:
            win = "tie"
        else:
            win = "Brigadier" if (b > p) == (better == "more") else "Plain"
        lines.append(f"| {name} | {bs} | {ps} | {win} |")
    return "\n".join(lines)


def main():
    a10score = load("a10score", os.path.join(HERE, "a10score.py"))
    plain = load("plain", os.path.join(HERE, "plain.py"))
    brig_run, plain_run = sys.argv[1], sys.argv[2]
    brig, pl = a10score.score(brig_run), plain.score_run(plain_run)  # rescored from the run dirs
    print(table(brig, pl))
    if "--json" in sys.argv:
        json.dump({"rows": [list(r) for r in rows(brig, pl)], "brigadier": brig_run, "plain": plain_run},
                  open(sys.argv[sys.argv.index("--json") + 1], "w"), indent=1)


if __name__ == "__main__":
    main()

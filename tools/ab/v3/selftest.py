#!/usr/bin/env python3
"""The V3 scorer's selftest.
1. Rules on fixed inputs: file:line citations, links, shell write targets, "To check" sections, the
   ask files' fact regexes against their known answers and against wrong ones.
2. Determinism, for every recorded run given (a run dir or a harness root): score.py twice in
   place, and once on a copy of the run dir somewhere else, give the same bytes.
usage: selftest.py [<root-or-run>...]   exits 1 on any difference or failed case"""
import glob, json, os, re, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import score, v3lib  # noqa: E402

failed = []


def case(name, got, want):
    if got != want:
        failed.append(name); print(f"FAIL {name}: got {got!r}, want {want!r}")


# 1. rules
c = score.cited("Set in `src/cli.ts:64`; see [tax.ts:9](/tmp/x/src/tax.ts#L9), lines 14-19 of src/cart.ts, "
                "and `src/dates.ts` line 16 or src/invoice.ts#L74.")
case("cited", c, [("/tmp/x/src/tax.ts", 9, 9), ("src/cart.ts", 14, 19), ("src/cli.ts", 64, 64), ("src/dates.ts", 16, 16),
                  ("src/invoice.ts", 74, 74), ("tax.ts", 9, 9)])
case("location in range", score.location_ok(c, {"file": "src/tax.ts", "lines": [9, 9]}), True)
case("location wrong line", score.location_ok([("src/tax.ts", 8, 8)], {"file": "src/tax.ts", "lines": [9, 9]}), False)
case("location too wide", score.location_ok([("src/tax.ts", 1, 32)], {"file": "src/tax.ts", "lines": [9, 9]}), False)
text = ("Wrote [rounding.md](/private/tmp/arm/data/scratch/t1/outputs/rounding.md) and file:///tmp/a/rounding.md#L3; "
        "also `rounding.md` alone, outputs/rounding.md relative, out-1a2b3c4d holds rounding.md, and /tmp/other/notes.md")
case("links", v3lib.links_to(text, "rounding.md"),
     ["/private/tmp/arm/data/scratch/t1/outputs/rounding.md", "/tmp/a/rounding.md", "out-1a2b3c4d"])
case("write targets", score.write_targets("cd /tmp/w && cp out.md ~/Desktop/out.md && cat x > y.md && echo hi | pbcopy", "/tmp/w"),
     ["~/Desktop/out.md", "/tmp/w/y.md"])
case("outside", [score.outside(p, "/tmp/w") for p in ("~/Desktop/a", "/tmp/w/a", "/Users/x/a", "/var/folders/q/a")],
     [True, False, True, False])
case("to check", len(score.to_check_sections("Done.\n\n**To check:** QC is 14% in the draft but 15% in src/tax.ts.\n\nBye")), 1)
case("save points", [score.points_at_save(t) for t in (
    "Use Save on the task card.", "You can also save that kept copy from the Outputs list on the task card.",
    "I saved it to your Desktop.", "Where would you like it? I can put it in docs/.")], [True, True, False, False])
known = {"q1": "Defaults to 30 days, set in `src/cli.ts:64`.", "q2": "`src/tax.ts:9`: QC standard is 0.15 (15%).",
         "q3": "`src/services.ts:7` sets `INVOICE_PREFIX = 'INV-'`.", "q4": "`src/invoice.ts:74`: `COL_NAME = 24`.",
         "s1": "`wip/discounts` is 2 commits ahead of `main`: they change `src/cart.ts` and `test/cart.test.ts`.",
         "s2": "Yes: all 81 tests pass (81/81, 0 failed).",
         "x1": "`parseCart` (src/cart.ts:65-80) uses `addItem`, whose `assertQty` (src/cart.ts:14-19) needs at least 1.",
         "x2": "`daysInMonth` (src/dates.ts:19-27) ignores leap years: `DAYS_PER_MONTH` (src/dates.ts:16) gives February 28 days."}
wrong = {"q1": "Defaults to 14 days, set in `src/cli.ts:64`.", "q2": "`src/tax.ts:8`: QC standard is 0.15.",
         "q3": "`src/services.ts:7` sets the prefix 'INV'.", "q4": "`src/invoice.ts:75`: 10.",
         "s1": "It is 3 commits ahead and changes src/cart.ts.", "s2": "Yes, they pass.",
         "x1": "The cart file is invalid JSON.", "x2": "The date format is wrong (src/cli.ts:44)."}
for ask in v3lib.ask_files(list(known)):
    case(f"{ask['id']} known answer passes", score.score_answer(ask["rule"], known[ask["id"]])[0], True)
    case(f"{ask['id']} wrong answer fails", score.score_answer(ask["rule"], wrong[ask["id"]])[0], False)
a6 = v3lib.ask_files(["a6"])[0]["rule"]
draft = open(os.path.join(HERE, "fixture", "overlay", "main", "docs", "rates-draft.md")).read()
case("a6 draft holds the planted error", bool(re.search(a6["planted"], draft)), True)
fixed = draft.replace("| QC | 14% |", "| QC | 15% |")
case("a6 fixed draft is right", bool(re.search(a6["right"], fixed)) and not re.search(a6["planted"], fixed), True)

class _Rec:
    cards = [{"kind": "question", "card": {"text": "Don't send rates.md yet: the draft says Quebec is 14%. What should I do?", "items": []}},
             {"kind": "question", "card": {"text": "Which name?", "items": [{"text": "Should the copy be called notes.md?"}]}},
             {"kind": "question-answer", "card": {"id": "x"}}]
case("a6 questions about the topic only", len(score.questions_about(_Rec, a6["topic"])), 1)

# 2. determinism
paths = sys.argv[1:]
runs = []
for p in paths:
    runs += [p] if os.path.exists(os.path.join(p, "freeze.json")) else sorted(
        r for r in glob.glob(os.path.join(p, "runs", "*")) if os.path.exists(os.path.join(r, "freeze.json")))
for run in runs:
    sc = lambda d: subprocess.run([sys.executable, os.path.join(HERE, "score.py"), d], capture_output=True, text=True).stdout
    one, two = sc(run), sc(run)
    with tempfile.TemporaryDirectory(prefix="v3-selftest-") as tmp:
        copy = os.path.join(tmp, os.path.basename(run))
        shutil.copytree(run, copy, symlinks=True, ignore=shutil.ignore_patterns("result", "checkdir"))
        three = sc(copy)
    case(f"deterministic {os.path.basename(run)}", (one == two, one == three, bool(one)), (True, True, True))
print(f"selftest: {len(failed)} failed; {len(runs)} recorded runs rescored three times")
sys.exit(1 if failed else 0)

#!/usr/bin/env python3
"""V1 overnight-style attempts (BUILD-PLAN E2): the one-phase "Invoice note" run (plan.json) on the
frozen V3 fixture, from Start through the morning report, with "Review the work" on or off, scored
against the hidden checks at the tip the run accepted (score.py).

It runs on a V3 harness root (tools/ab/v3/run.py template --root R): the same build, template,
fixture and node_modules cache, and the same arm paths, so `v3/run.py cleanup --root R` removes
everything it made too. Each attempt restores the seeded template, starts the arm's daemon and the
recorder (which answers cards like the user and never merges or pushes), sends a warm-up message
(the warm regime), sets reviews on or off, proposes and starts the run (overnight.py), waits for it
to finish and the conversation to go quiet, then stops the arm and scores.

  runs/v1o-<reviews>-<label>-<n>/  rec/, plan.json, overnight-start.json, start.json, times.json,
                                   tokens.log, score.json
  v1o-results.jsonl               one line per attempt (every attempt, failed or not)

usage: run.py attempt --root R --reviews on|off [--n 1] [--tag LABEL] [--timeout-min 120]
       run.py set --root R [--reruns 2]          (on, off, off, on, ... alternating)"""
import argparse, importlib.util, json, os, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
AB = os.path.dirname(HERE)
sys.path.insert(0, AB)
from bipc import req  # noqa: E402

_spec = importlib.util.spec_from_file_location("v3run", os.path.join(AB, "v3", "run.py"))
v3 = importlib.util.module_from_spec(_spec)
sys.path.insert(0, os.path.join(AB, "v3"))
_spec.loader.exec_module(v3)
sys.path.insert(0, HERE)
import score as scorer  # noqa: E402

QUIET_AFTER_S = 1800  # after the run finished: at most this long for the conversation to go quiet


def data_size(data):
    """The data dir's size and its stored outputs' (BUILD-PLAN D2's disk-growth measure), in bytes."""
    def size(d):
        total = files = 0
        for dirpath, _, names in os.walk(d):
            for f in names:
                try:
                    total += os.lstat(os.path.join(dirpath, f)).st_size; files += 1
                except OSError:
                    pass
        return total, files
    (data_bytes, _), (blob_bytes, blob_files) = size(data), size(os.path.join(data, "blobs"))
    return {"data_bytes": data_bytes, "blob_bytes": blob_bytes, "blob_files": blob_files}


def attempt(root, reviews, n, tag, timeout_min):
    build = json.load(open(os.path.join(root, "build.json")))
    label = tag or build["label"]
    name = f"v1o-{reviews}-{label}-{n}"
    run = os.path.join(root, "runs", name)
    if os.path.exists(os.path.join(run, "score.json")):
        v3.log(name, "scored already"); return json.load(open(os.path.join(run, "score.json")))
    if os.path.exists(run):
        v3.safe_rmtree(root, run)
    os.makedirs(run)
    shutil.copyfile(os.path.join(HERE, "plan.json"), os.path.join(run, "plan.json"))
    v3.restore(root, "seeded")
    disk = {"before": data_size(os.path.join(root, "arm", "data"))}
    arm = v3.Arm(root, os.path.join(run, "rec"), run)
    start = {"arm": "brigadier", "task": "v1-overnight", "reviews": reviews, "n": n, "build": build, "label": label,
             "regime": "warm"}
    finished = False
    try:
        time.sleep(5)  # the start-up index scan
        wconv, _ = v3.create_conversation(arm.data, arm.repo)
        _, wrid = v3.send(arm.data, wconv, v3.WARMUP_TEXT)
        v3.wait_request(arm.events, wconv, wrid, time.time() + 600)
        v3.settle(arm.events, wconv, time.time() + 300)
        s = req(arm.data, {"method": "getCatalog"})["value"]["catalog"]["settings"]
        s["reviewWork"] = reviews == "on"
        v3.log(name, f"reviews {reviews}:", req(arm.data, {"method": "updateSettings", "settings": s})["status"])
        with open(os.path.join(run, "overnight.log"), "w") as out:
            p = subprocess.run([sys.executable, os.path.join(AB, "overnight.py"), arm.data, arm.repo,
                                os.path.join(run, "plan.json"), arm.events, "--timeout-min", str(timeout_min)],
                               stdout=out, stderr=subprocess.STDOUT)
        finished = p.returncode == 0
        on = json.load(open(os.path.join(run, "overnight-start.json")))
        start.update(conversation=on["conversation"], run=on["run"], t0_ms=on["t0_ms"], finished=finished)
        json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
        v3.log(name, "run", "finished" if finished else "did not finish")
        start["settled_quietly"] = v3.settle(arm.events, on["conversation"], time.time() + QUIET_AFTER_S)
        json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
    finally:
        arm.close()
    disk["after"] = data_size(os.path.join(root, "arm", "data"))
    start["disk"] = disk
    json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
    try:
        v3.after(run)
    except Exception as err:  # noqa: BLE001  (usage is kept apart from the verdict)
        v3.log(name, "times/tokens:", repr(err))
    result = scorer.score(run, arm.repo, reviews, os.path.join(root, "nm-cache", "node_modules"))
    result["finished"] = finished
    json.dump(result, open(os.path.join(run, "score.json"), "w"), indent=1, sort_keys=True)
    with open(os.path.join(root, "v1o-results.jsonl"), "a") as f:
        f.write(json.dumps({"run": name, "root": root, **{k: result[k] for k in (
            "pass", "completed", "why", "outcome", "verified_commit", "reviews", "review", "false_verified",
            "missing_evidence", "finished")}}, sort_keys=True) + "\n")
    v3.log(name, "PASS" if result["pass"] else "FAIL", "completed" if result["completed"] else "not completed", result["why"])
    return result


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for cmd in ("attempt", "set"):
        p = sub.add_parser(cmd)
        p.add_argument("--root", required=True); p.add_argument("--tag")
        p.add_argument("--timeout-min", type=float, default=120)
        if cmd == "attempt":
            p.add_argument("--reviews", choices=["on", "off"], required=True); p.add_argument("--n", type=int, default=1)
        else:
            p.add_argument("--reruns", type=int, default=2)
    a = ap.parse_args()
    root = v3.check_root(a.root)
    if not os.path.isdir(os.path.join(root, "template")):
        sys.exit(f"run.py: {root} has no template: run tools/ab/v3/run.py template --root {a.root} first")
    if a.cmd == "attempt":
        attempt(root, a.reviews, a.n, a.tag, a.timeout_min)
        return
    order = []
    for n in range(1, a.reruns + 1):
        order += [("on", n), ("off", n)] if n % 2 else [("off", n), ("on", n)]
    for reviews, n in order:
        try:
            attempt(root, reviews, n, a.tag, a.timeout_min)
        except Exception as err:  # noqa: BLE001  (a harness error is a failed attempt; the set goes on)
            v3.log(f"v1o-{reviews}-{n}", "HARNESS ERROR", repr(err))
            with open(os.path.join(root, "v1o-results.jsonl"), "a") as f:
                f.write(json.dumps({"run": f"v1o-{reviews}-{n}", "root": root, "reviews": reviews, "pass": False,
                                    "completed": False, "why": f"harness error: {err!r}"}) + "\n")


if __name__ == "__main__":
    main()

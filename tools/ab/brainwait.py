#!/usr/bin/env python3
"""Waits until the Brain jobs that adding the arm's project started (index, skeleton) have ended,
so a Brigadier arm's clock starts on a settled daemon, as in the 10-05 runs. Reads the recorder's
events; needs at least one Brain job seen (gives up after --max seconds without one).
usage: brainwait.py <arm-dir> [--max 600]"""
import argparse, json, os, sys, time

ap = argparse.ArgumentParser()
ap.add_argument("arm"); ap.add_argument("--max", type=int, default=600)
a = ap.parse_args()
evf = os.path.join(a.arm, "rec", "events.jsonl")
start = time.time()
while True:
    jobs = {}
    if os.path.exists(evf):
        for line in open(evf):
            e = json.loads(line)["event"]
            if e.get("type") == "brainJobUpdated":
                j = e["job"]
                jobs[j["id"]] = (j["kind"], j["state"]["type"])
    running = [v for v in jobs.values() if v[1] in ("running", "queued", "pending")]
    print(time.strftime("%T"), jobs, flush=True)
    if jobs and not running:
        break
    if not jobs and time.time() - start > a.max:
        sys.exit("brainwait: no Brain job seen")
    time.sleep(10)

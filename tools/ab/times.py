#!/usr/bin/env python3
"""An arm's time boundaries, in ms and as seconds after submit (t0):
- landed: the verified landing (THREAD-PLAN §3). Brigadier: the request's last `landed` step.
  /delegator: the later of (a) the moment the last checking worker went `done` (its status file's
  mtime; the verifier, or the lead when no verifier ran) and (b) the reflog time at which the
  result tip reached the result branch. Pass --tip/--branch (from the run's REPORT) and, if the
  run had several checkers, --checker wNN. "Verified" also needs the frozen checks to pass on that
  tip; check.sh records that, this script only times it.
- answer: the final answer. Brigadier: the request's end (`endedAtMs`). /delegator: the
  coordinator's last end-of-turn message in its transcript (pass --answer-before to cut off a
  later user follow-up).
- settled: the last activity of anything the request started (Brigadier: the conversation's last
  event or usage row; /delegator: the last line of any session in the manifest), the end of the
  token window.
usage: times.py brigadier <arm-dir>
       times.py delegator <arm-dir> --tip SHA --branch NAME [--checker wNN] [--manifest m.json]
                [--answer-before MS]"""
import argparse, datetime, glob, json, os, sqlite3, subprocess, sys

ap = argparse.ArgumentParser()
ap.add_argument("kind", choices=["brigadier", "delegator"]); ap.add_argument("arm")
ap.add_argument("--tip"); ap.add_argument("--branch"); ap.add_argument("--checker")
ap.add_argument("--manifest"); ap.add_argument("--answer-before", type=int)
a = ap.parse_args()
st = json.load(open(os.path.join(a.arm, "start.json"))); t0 = st["t0_ms"]
H = os.path.expanduser("~")


def ts(s):
    return int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def row(landed, answer, settled, extra):
    sec = lambda v: None if v is None else round((v - t0) / 1000, 1)
    return {"t0_ms": t0, "landed_ms": landed, "answer_ms": answer, "settled_ms": settled,
            "landed_s": sec(landed), "answer_s": sec(answer), "settled_s": sec(settled), **extra}


if a.kind == "brigadier":
    conv = st["conversation"]; request = None; landed = None; answer = None; last = t0; heads = []
    for line in open(os.path.join(a.arm, "rec", "events.jsonl")):
        env = json.loads(line); e = env["event"]
        if not env.get("stream", "").endswith(conv) and env.get("stream", "").split(":")[0] not in ("task", "orch"):
            continue
        if e["type"] == "requestUpdated" and e["request"]["conversationId"] == conv:
            if request is None and e["request"]["startedAtMs"] >= t0 - 2000:
                request = e["request"]["id"]
            if e["request"]["id"] == request and e["request"].get("endedAtMs"):
                answer = e["request"]["endedAtMs"]
        if e["type"] == "orchestratorStepped" and e["step"]["kind"]["type"] == "landed" and e["step"].get("requestId") == request:
            landed = env["atMs"]; heads.append(e["step"]["kind"]["head"])
        if env["atMs"] >= t0:
            last = max(last, env["atMs"])
    db = sqlite3.connect(f"file:{os.path.join(a.arm, 'data', 'routing.sqlite')}?mode=ro", uri=True)
    usage_last = db.execute("SELECT MAX(at_ms) FROM turn_usage WHERE conversation_id = ?", (conv,)).fetchone()[0]
    print(json.dumps(row(landed, answer, max(last, usage_last or 0), {"request": request, "landed_heads": heads}), indent=1))
    sys.exit(0)

# /delegator
repo = os.path.join(a.arm, "repo")
man = json.load(open(a.manifest)) if a.manifest else None
run = man["run"] if man else None
checker = None
if run:
    done = []
    for w in sorted(glob.glob(run + "/workers/w*")):
        status = os.path.join(w, "status")
        if os.path.exists(status) and open(status).read().strip() == "done":
            title = open(w + "/title").read().strip() if os.path.exists(w + "/title") else ""
            done.append((os.path.basename(w), title, int(os.path.getmtime(status) * 1000)))
    pick = [d for d in done if d[0] == a.checker] if a.checker else [d for d in done if "verif" in d[1].lower()] or done
    checker = max(pick, key=lambda d: d[2]) if pick else None
reached = None
if a.tip and a.branch:
    out = subprocess.run(["git", "-C", repo, "reflog", "show", "--date=unix", "--format=%H %gd", a.branch],
                         capture_output=True, text=True).stdout.splitlines()
    for line in reversed(out):  # oldest first
        sha, sel = line.split(" ", 1)
        if subprocess.run(["git", "-C", repo, "merge-base", "--is-ancestor", a.tip, sha]).returncode == 0:
            reached = int(sel.split("@{")[1].rstrip("}")) * 1000
            break
landed = max(x for x in (checker[2] if checker else None, reached) if x is not None) if (checker or reached) else None
answer = None; settled = t0
if man:
    for s in man["sessions"]:
        if s["provider"] != "claude":
            continue
        for f in glob.glob(f"{H}/.claude/projects/*/{s['id']}.jsonl"):
            for line in open(f, errors="replace"):
                try:
                    r = json.loads(line)
                except Exception:
                    continue
                if not r.get("timestamp"):
                    continue
                t = ts(r["timestamp"]); settled = max(settled, t)
                m = r.get("message") or {}
                if s["role"] == "coordinator" and r.get("type") == "assistant" and m.get("stop_reason") == "end_turn":
                    if a.answer_before is None or t < a.answer_before:
                        answer = max(answer or 0, t)
    for s in man["sessions"]:
        if s["provider"] == "codex":
            for f in glob.glob(f"{H}/.codex/sessions/*/*/*/rollout-*{s['id']}.jsonl"):
                for line in open(f):
                    r = json.loads(line)
                    if r.get("timestamp"):
                        settled = max(settled, ts(r["timestamp"]))
print(json.dumps(row(landed, answer, settled, {"checker": checker, "tip_reached_branch_ms": reached,
                                                "tip": a.tip, "branch": a.branch}), indent=1))

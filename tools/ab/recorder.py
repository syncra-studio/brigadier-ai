#!/usr/bin/env python3
"""Records every committed daemon event (Subscribe + EventsSince backfill, deduped by seq) and
answers any card the way the user would, logging it so it is counted. The session's merge card
(finish_session) is logged but left alone: the user's merge click is outside the measured span.
With --merge it is approved too. Session transcripts are hard-linked into <out-dir>/../transcripts
as soon as they exist, because Brigadier deletes them when a task is cleaned up.
usage: recorder.py <data-dir> <out-dir> [--merge]"""
import json, os, sys, time, threading
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bipc import connect, frame, read, call
data, out = sys.argv[1], sys.argv[2]
merge = "--merge" in sys.argv[3:]
os.makedirs(out, exist_ok=True)
evf = open(os.path.join(out, "events.jsonl"), "a")
cardf = open(os.path.join(out, "cards.jsonl"), "a")
seen = set(); last = [0]
answered = set()
def now(): return int(time.time() * 1000)
def log_card(kind, obj, action):
    cardf.write(json.dumps({"recv_ms": now(), "kind": kind, "action": action, "card": obj}) + "\n"); cardf.flush()
def answer(req):
    try:
        s, _ = connect(data); r = call(s, req); s.close(); return r
    except Exception as e:
        return {"error": str(e)}
def handle(env):
    ev = env["event"]; t = ev.get("type")
    if t == "cleanupRecorded":
        a = ev["artifact"]
        if a["type"] == "claudeSession": sessions.add(("claude", a["sessionId"]))
        if a["type"] == "codexThread": sessions.add(("codex", a["threadId"]))
    if t == "workerEvent" and ev["event"].get("type") == "sessionStarted":
        nid = ev["event"].get("nativeId") or ""
        sessions.add(("claude", nid)); sessions.add(("codex", nid))
    if t == "approvalUpdated":
        a = ev["approval"]
        if a["state"].get("type") == "pending" and a["id"] not in answered:
            answered.add(a["id"])
            if a["subject"].get("type") == "finishSession" and not merge:
                log_card("merge-card", a, "left for the user")
                return
            log_card("approval", a, "allow")
            threading.Thread(target=lambda: log_card("approval-answer", {"id": a["id"]}, answer(
                {"method": "answerCard", "conversationId": a["conversationId"], "cardId": a["id"],
                 "decision": {"type": "allow"}})), daemon=True).start()
    elif t == "questionUpdated":
        q = ev["question"]
        if q.get("answer") is None and q["id"] not in answered:
            answered.add(q["id"])
            opts = q.get("options") or []
            rec = q.get("recommended")
            text = opts[rec] if (rec is not None and rec < len(opts)) else "Go with your recommendation."
            log_card("question", q, text)
            threading.Thread(target=lambda: log_card("question-answer", {"id": q["id"]}, answer(
                {"method": "answerQuestion", "conversationId": q["conversationId"], "cardId": q["id"],
                 "answer": text})), daemon=True).start()
    elif t == "planUpdated":
        p = ev["plan"]
        if p["state"].get("type") == "proposed" and p["id"] not in answered:
            answered.add(p["id"])
            log_card("plan", p, "approve")
            threading.Thread(target=lambda: log_card("plan-answer", {"id": p["id"]}, answer(
                {"method": "decidePlan", "conversationId": p["conversationId"], "cardId": p["id"],
                 "approve": True, "message": None})), daemon=True).start()
def record(env):
    if env["seq"] in seen: return
    seen.add(env["seq"]); last[0] = max(last[0], env["seq"])
    evf.write(json.dumps({"recv_ms": now(), **env}) + "\n"); evf.flush()
    handle(env)
def backfill():
    s, _ = connect(data)
    while True:
        r = call(s, {"method": "eventsSince", "afterSeq": last[0], "limit": 500})
        evs = r["value"]["events"]
        for e in sorted(evs, key=lambda e: e["seq"]): record(e)
        if len(evs) < 500: break
    s.close()
import glob
H = os.path.expanduser("~"); keep = os.path.join(out, "..", "transcripts")
sessions = set()
parent_of = {}
def codex_children(sid):
    """Rollouts of the Codex threads `sid` started (a review's, an auto-review's), which name it
    as `parent_thread_id` in their first line."""
    for f in glob.glob(f"{H}/.codex/sessions/*/*/*/rollout-*.jsonl"):
        if f not in parent_of:
            try: parent_of[f] = json.loads(open(f).readline())["payload"].get("parent_thread_id")
            except Exception: continue
        if parent_of[f] == sid: yield f
def keeper():
    """Hard-links every session transcript/rollout as soon as it exists: Brigadier deletes
    them when a task is cleaned up, and a hard link keeps the file (appends included)."""
    while True:
        for kind, sid in list(sessions):
            if kind == "claude":
                files = [(f, f"claude/{sid}.jsonl") for f in glob.glob(f"{H}/.claude/projects/*/{sid}.jsonl")]
                files += [(f, f"claude/{sid}/subagents/{os.path.basename(f)}") for f in glob.glob(f"{H}/.claude/projects/*/{sid}/subagents/*.jsonl")]
            else:
                files = [(f, f"codex/{os.path.basename(f)}") for f in glob.glob(f"{H}/.codex/sessions/*/*/*/rollout-*{sid}.jsonl")]
                files += [(f, f"codex/{os.path.basename(f)}") for f in codex_children(sid)]
            for src, rel in files:
                dst = os.path.join(keep, rel)
                if not os.path.exists(dst):
                    os.makedirs(os.path.dirname(dst), exist_ok=True)
                    try: os.link(src, dst)
                    except Exception as e: print("link", src, e, flush=True)
        time.sleep(2)
threading.Thread(target=keeper, daemon=True).start()
while True:
    try:
        backfill()
        s, welcome = connect(data)
        frame(s, {"type": "request", "id": 1, "request": {"method": "subscribe", "afterSeq": last[0], "metrics": False}})
        while True:
            f = read(s)
            if f is None: raise ConnectionError("closed")
            if f.get("type") == "event": record(f["event"])
            elif f.get("type") == "lagged":
                print("lagged", f, flush=True); break
            elif f.get("type") == "closing": raise ConnectionError("daemon closing")
        s.close()
    except Exception as e:
        print("recorder:", e, flush=True); time.sleep(2)

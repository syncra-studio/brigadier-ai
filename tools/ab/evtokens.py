#!/usr/bin/env python3
"""Token usage per worker session from the daemon's own recorded worker events (Brigadier
normalizes: inputTokens = uncached input; cached and cache-write separate; reasoning inside
output). Codex: the last cumulative `usage.total` of each session. Claude: the sum of each
turn's `turnCompleted.usage`. usage: evtokens.py <arm-dir> [end_ms]"""
import json, sys, collections
arm = sys.argv[1]; end = int(sys.argv[2]) if len(sys.argv) > 2 else 1 << 62
tasks = {}; sess = {}; cur = {}
for l in open(arm + "/rec/events.jsonl"):
    env = json.loads(l); e = env["event"]
    if env["atMs"] > end: continue
    if e["type"] == "taskUpdated": tasks[e["task"]["id"]] = e["task"]
    if e["type"] != "workerEvent": continue
    tid = e["taskId"]; w = e["event"]; t = w.get("type")
    if t == "sessionStarted":
        cur[tid] = w["nativeId"]; sess.setdefault(w["nativeId"], {"task": tid, "model": w.get("model"), "codex_total": None, "claude_turns": collections.Counter()})
    s = sess.get(cur.get(tid))
    if not s: continue
    if t == "usage" and w.get("total"): s["codex_total"] = w["total"]
    if t == "turnCompleted" and w.get("usage"):
        s["claude_turns"].update({k: v for k, v in w["usage"].items() if isinstance(v, int)})
grand = collections.Counter()
for sid, s in sess.items():
    t = tasks.get(s["task"], {}); prov = t.get("route", {}).get("choice", {}).get("provider")
    u = s["codex_total"] if prov == "codex" else dict(s["claude_turns"])
    if not u: print("UNKNOWN", sid, prov); continue
    raw = sum(u.get(k) or 0 for k in ("inputTokens", "cachedInputTokens", "cacheWriteTokens", "outputTokens"))
    grand[prov + "_raw"] += raw
    for k in ("inputTokens", "cachedInputTokens", "cacheWriteTokens", "outputTokens", "reasoningTokens"):
        grand[f"{prov}_{k}"] += u.get(k) or 0
    print(f"{t.get('kind','?'):10} {prov:6} {s['model']} {t.get('title','')[:40]!r:42} raw={raw} {json.dumps({k:u.get(k) for k in ('inputTokens','cachedInputTokens','cacheWriteTokens','outputTokens','reasoningTokens')})}")
print("WORKERS TOTAL", dict(grand))

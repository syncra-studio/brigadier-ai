#!/usr/bin/env python3
"""Token accounting over an explicit session manifest.
manifest: JSON list of {"label","role","provider":"claude"|"codex","id"} ; window [start_ms,end_ms]
Claude: finalized usage per assistant message id (the record with the largest output count is the
final one; repeated records of one message carry the same input/cache numbers). Subagent
transcripts under <session>/subagents/ are included and listed.
Codex: cumulative total_token_usage; cached input is a subset of input, reasoning is inside output.
usage: tokens.py <manifest.json> <start_ms> <end_ms> [--json out]
The manifest is a list, or dlg_manifest.py's object with "sessions"."""
import json, sys, glob, os, collections, datetime
man = json.load(open(sys.argv[1])); man = man["sessions"] if isinstance(man, dict) else man; start = int(sys.argv[2]); end = int(sys.argv[3])
H = os.path.expanduser("~")
def tsms(s): return int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
KEEP = os.environ.get("KEEP")  # an arm's transcripts/ folder of hard links, searched first
def claude_files(sid):
    main = glob.glob(f"{KEEP}/claude/{sid}.jsonl") if KEEP else []
    if main:
        return main, glob.glob(f"{KEEP}/claude/{sid}/subagents/*.jsonl")
    main = glob.glob(f"{H}/.claude/projects/*/{sid}.jsonl")
    subs = []
    for m in main: subs += glob.glob(m[:-6] + "/subagents/*.jsonl")
    return main, subs
def claude_usage(path):
    best = {}
    for l in open(path):
        try: r = json.loads(l)
        except Exception: continue
        if r.get("type") != "assistant": continue
        m = r.get("message") or {}; u = m.get("usage")
        if not u or not m.get("id"): continue
        t = tsms(r["timestamp"]) if r.get("timestamp") else None
        if t is not None and not (start <= t <= end): continue
        prev = best.get(m["id"])
        if prev is None or (u.get("output_tokens") or 0) >= (prev[1].get("output_tokens") or 0):
            best[m["id"]] = (m.get("model"), u)
    tot = collections.Counter(); by = collections.defaultdict(collections.Counter)
    for model, u in best.values():
        c = collections.Counter(input=u.get("input_tokens") or 0, cache_read=u.get("cache_read_input_tokens") or 0,
                                cache_write=u.get("cache_creation_input_tokens") or 0, output=u.get("output_tokens") or 0, calls=1)
        tot.update(c); by[model].update(c)
    return tot, by
def codex_usage(tid):
    fs = (glob.glob(f"{KEEP}/codex/rollout-*{tid}.jsonl") if KEEP else []) or glob.glob(f"{H}/.codex/sessions/*/*/*/rollout-*{tid}.jsonl")
    if not fs: return None, None, fs
    before = None; last = None; model = None
    for l in open(fs[0]):
        try: r = json.loads(l)
        except Exception: continue
        p = r.get("payload") or {}
        if r.get("type") == "turn_context": model = p.get("model") or model
        if p.get("type") == "token_count" and p.get("info"):
            t = tsms(r["timestamp"]); u = p["info"]["total_token_usage"]
            if t < start: before = u
            elif t <= end: last = u
    if last is None: return collections.Counter(), model, fs
    b = before or {}
    c = collections.Counter({k: (last.get(k) or 0) - (b.get(k) or 0) for k in
                             ("input_tokens", "cached_input_tokens", "output_tokens", "reasoning_output_tokens")})
    return c, model, fs
rows = []; grand = collections.Counter(); bymodel = collections.defaultdict(collections.Counter); unknown = []
for s in man:
    if s["provider"] == "claude":
        mains, subs = claude_files(s["id"])
        if not mains: unknown.append(s); rows.append((s, None, "transcript missing")); continue
        tot = collections.Counter(); 
        for f in mains + subs:
            t, by = claude_usage(f); tot.update(t)
            for k, v in by.items(): bymodel["claude:" + str(k)].update(v)
        raw = tot["input"] + tot["cache_read"] + tot["cache_write"] + tot["output"]
        grand.update(claude_raw=raw, claude_input=tot["input"], claude_cache_read=tot["cache_read"], claude_cache_write=tot["cache_write"], claude_output=tot["output"])
        rows.append((s, dict(tot, raw=raw, subagents=len(subs)), None))
    else:
        c, model, fs = codex_usage(s["id"])
        if c is None: unknown.append(s); rows.append((s, None, "rollout missing")); continue
        raw = c["input_tokens"] + c["output_tokens"]
        grand.update(codex_raw=raw, codex_input=c["input_tokens"], codex_cached=c["cached_input_tokens"], codex_output=c["output_tokens"], codex_reasoning=c["reasoning_output_tokens"])
        bymodel["codex:" + str(model)].update(input=c["input_tokens"], cached=c["cached_input_tokens"], output=c["output_tokens"])
        rows.append((s, dict(c, raw=raw, model=model), None))
for s, u, err in rows:
    print(f"{s.get('role',''):12} {s['label'][:38]:38} {s['provider']:6} {s['id'][:36]:36} " + (err or json.dumps(u)))
grand["total_raw"] = grand["claude_raw"] + grand["codex_raw"]
print("BY MODEL:"); [print(" ", k, dict(v)) for k, v in bymodel.items()]
print("TOTAL:", dict(grand)); print("UNKNOWN sessions:", len(unknown), [u["label"] for u in unknown])
if "--json" in sys.argv:
    json.dump({"rows": [{"session": s, "usage": u, "error": e} for s, u, e in rows], "by_model": {k: dict(v) for k, v in bymodel.items()},
               "total": dict(grand), "unknown": unknown}, open(sys.argv[sys.argv.index("--json") + 1], "w"), indent=1)

#!/usr/bin/env python3
"""Session audit for one arm: every Claude session and Codex thread on this machine that started
between t0 and the end, set against the arm's manifest. Each one not counted for the arm must be
explained in <arm>/audit-other.json ({"<id>": "<why it isn't the arm's>"}, e.g. the Delegator's
own coordinator, the w24 measuring session, the evaluator's probes). An unexplained one makes the
evidence incomplete: it is listed and the script exits 3.
Sessions a daemon deleted are seen through snap.py's links in <arm>/snap/ (run it during the arm).
usage: audit.py <arm-dir> <end_ms>   (reads <arm>/manifest.json, list or {"sessions": [...]})"""
import datetime, glob, json, os, sys

arm = sys.argv[1]; end = int(sys.argv[2]); H = os.path.expanduser("~")
t0 = json.load(open(os.path.join(arm, "start.json")))["t0_ms"]
man = json.load(open(os.path.join(arm, "manifest.json")))
man = man["sessions"] if isinstance(man, dict) else man
counted = {s["id"] for s in man}
other_path = os.path.join(arm, "audit-other.json")
other = json.load(open(other_path)) if os.path.exists(other_path) else {}


def ms(s):
    return int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def claude_first(f):
    """(start ms, cwd, first user text) of a Claude transcript."""
    start = cwd = text = None
    for i, line in enumerate(open(f, errors="replace")):
        if i > 200: break
        try: r = json.loads(line)
        except ValueError: continue
        start = start or (ms(r["timestamp"]) if r.get("timestamp") else None)
        cwd = cwd or r.get("cwd")
        if text is None and r.get("type") == "user":
            c = (r.get("message") or {}).get("content")
            text = c if isinstance(c, str) else json.dumps(c)[:300]
        if start and cwd and text: break
    return start, cwd, (text or "")[:160].replace("\n", " ")


rows = []; done_ids = set()
snap = os.path.join(arm, "snap")  # snap.py's hard links: sessions a daemon deleted afterwards
claude_files = glob.glob(f"{H}/.claude/projects/*/*.jsonl") + glob.glob(f"{snap}/claude/*.jsonl")
codex_files = glob.glob(f"{H}/.codex/sessions/*/*/*/rollout-*.jsonl") + glob.glob(f"{snap}/codex/rollout-*.jsonl")
for f in claude_files:
    if os.path.basename(f)[:-6] in done_ids: continue
    done_ids.add(os.path.basename(f)[:-6])
    if os.path.getmtime(f) * 1000 < t0: continue
    start, cwd, text = claude_first(f)
    if start is None or not (t0 <= start <= end): continue
    rows.append(("claude", os.path.basename(f)[:-6], start, cwd, text))
for f in codex_files:
    if os.path.getmtime(f) * 1000 < t0: continue
    try: p = json.loads(open(f).readline())["payload"]
    except (ValueError, KeyError): continue
    if p["id"] in done_ids: continue
    done_ids.add(p["id"])
    start = ms(p.get("timestamp", "1970-01-01T00:00:00Z"))
    if t0 <= start <= end:
        rows.append(("codex", p["id"], start, p.get("cwd"), f"parent={p.get('parent_thread_id')} source={p.get('source')}"))
unexplained = []
for kind, sid, start, cwd, text in sorted(rows, key=lambda r: r[2]):
    tag = "counted" if sid in counted else ("other: " + other[sid]) if sid in other else "UNEXPLAINED"
    print(f"{tag[:60]:60} {kind:6} {sid} +{(start - t0) / 1000:7.1f}s {cwd} | {text}")
    if tag == "UNEXPLAINED": unexplained.append(sid)
missing = [s["id"] for s in man if s["id"] not in {r[1] for r in rows}]
print(f"{len(rows)} sessions started in the window; counted {sum(r[1] in counted for r in rows)}, "
      f"explained {sum(r[1] in other for r in rows)}, unexplained {len(unexplained)}; "
      f"{len(missing)} manifest sessions started outside the window or not found: {missing}")
if unexplained:
    print("audit: INCOMPLETE: unexplained sessions above", file=sys.stderr); sys.exit(3)

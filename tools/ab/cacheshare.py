#!/usr/bin/env python3
"""How much of a Claude session's first call came from the cache (THREAD-PLAN §3 phase 3: a
second worker reads ≥ 50%). For each transcript given (a `~/.claude/projects/…/<id>.jsonl`, or a
directory of them), in the order their first call was made: the first assistant message's
usage, and cache_read / (input + cache_read + cache_creation).
usage: cacheshare.py <transcript.jsonl|dir>... [--json]"""
import glob, json, os, sys

paths = []
for arg in sys.argv[1:]:
    if arg == "--json":
        continue
    paths += sorted(glob.glob(os.path.join(arg, "*.jsonl"))) if os.path.isdir(arg) else [arg]

rows = []
for path in paths:
    for line in open(path):
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        msg = rec.get("message") or {}
        if rec.get("type") != "assistant" or not isinstance(msg.get("usage"), dict):
            continue
        u = msg["usage"]
        inp = u.get("input_tokens", 0)
        read = u.get("cache_read_input_tokens", 0)
        write = u.get("cache_creation_input_tokens", 0)
        total = inp + read + write
        rows.append({"transcript": os.path.basename(path), "at": rec.get("timestamp"),
                     "cwd": rec.get("cwd"), "input": inp, "cache_read": read,
                     "cache_write": write, "share": round(read / total, 3) if total else None})
        break
rows.sort(key=lambda r: r["at"] or "")
if "--json" in sys.argv:
    print(json.dumps(rows, indent=1))
else:
    for r in rows:
        print(f"{r['at']} {r['transcript']}: input {r['input']}, cache read {r['cache_read']}, "
              f"cache write {r['cache_write']} -> {r['share']} from cache")
